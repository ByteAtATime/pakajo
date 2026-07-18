use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use anyhow::Context as _;

use crate::events::{AurDepSource, InstallEvent, InstallSink};
use crate::resolve::BuildPlan;

pub fn run_build<S: InstallSink + ?Sized>(
    target: &str,
    no_check: bool,
    user_as_deps: bool,
    sink: &mut S,
    confirm: impl FnOnce(&BuildPlan) -> bool,
) -> anyhow::Result<()> {
    sink.event(InstallEvent::ResolvingAurDependencies {
        target: target.to_string(),
    });

    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let alpm = crate::pacman::init_alpm(&config)?;
    let aur = crate::aur::AurClient::new();
    let plan = crate::resolve::resolve(
        &crate::resolve::AlpmDb(&alpm),
        &aur,
        &[target.to_string()],
        no_check,
    )?;

    let aur_packages: usize = plan.layers.iter().map(|l| l.aur.len()).sum();
    let repo_deps: usize = plan.layers.iter().map(|l| l.repo_deps.len()).sum();
    for layer in &plan.layers {
        for name in &layer.repo_deps {
            sink.event(InstallEvent::AurDepResolved {
                package: name.clone(),
                source: AurDepSource::Repo,
            });
        }
        for info in &layer.aur {
            sink.event(InstallEvent::AurDepResolved {
                package: info.name.clone(),
                source: AurDepSource::Aur,
            });
        }
    }
    sink.event(InstallEvent::ResolutionComplete {
        layers: plan.layers.len(),
        aur_packages,
        repo_deps,
    });

    if !confirm(&plan) {
        anyhow::bail!("build cancelled by user");
    }

    let total_layers = plan.layers.len();
    for (idx, layer) in plan.layers.iter().enumerate() {
        sink.event(InstallEvent::LayerBoundary {
            layer: idx,
            total: total_layers,
        });

        if !layer.repo_deps.is_empty() {
            run_install_child(&layer.repo_deps, true, sink)?;
        }

        for info in &layer.aur {
            let pkgbase = &info.package_base;
            let dir = clone_dir(pkgbase)?;

            sink.event(InstallEvent::CloningRepo {
                package: info.name.clone(),
            });
            git_clone_or_pull(&dir, pkgbase)?;

            sink.event(InstallEvent::BuildStarted {
                package: info.name.clone(),
            });
            run_makepkg_streaming(&dir, no_check, &info.name, sink)?;

            let expected = expected_artifacts(&dir)
                .with_context(|| format!("failed to enumerate artifacts for {}", info.name))?;
            let artifacts = collect_artifacts(&dir, &expected)?;
            sink.event(InstallEvent::BuildCompleted {
                package: info.name.clone(),
                artifacts: artifacts.clone(),
            });

            let as_deps = user_as_deps || !plan.targets.iter().any(|t| t == &info.name);
            run_install_child(&artifacts, as_deps, sink)?;
        }
    }

    Ok(())
}

fn collect_artifacts(dir: &Path, expected: &[String]) -> anyhow::Result<Vec<String>> {
    let mut artifacts: Vec<String> = Vec::with_capacity(expected.len());
    for basename in expected {
        let path = dir.join(basename);
        if !path.exists() {
            anyhow::bail!("expected artifact not found: {}", path.display());
        }
        artifacts.push(path.to_string_lossy().into_owned());
    }
    Ok(artifacts)
}

fn is_valid_pkgbase(s: &str) -> Option<()> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')) {
            return None;
        }
    }
    Some(())
}

fn clone_dir(pkgbase: &str) -> anyhow::Result<PathBuf> {
    if is_valid_pkgbase(pkgbase).is_none() {
        anyhow::bail!("invalid pkgbase from AUR: {pkgbase:?}");
    }
    let cache = match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => PathBuf::from(xdg),
        Err(_) => {
            let home =
                std::env::var("HOME").context("no cache directory: set XDG_CACHE_HOME or HOME")?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(cache.join("pakajo").join(pkgbase))
}

fn git_clone_or_pull(dir: &Path, pkgbase: &str) -> anyhow::Result<()> {
    if dir.exists() {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["pull", "--ff-only"])
            .stdout(Stdio::null())
            .status()
            .with_context(|| format!("failed to run git pull for {pkgbase}"))?;
        if !status.success() {
            anyhow::bail!("git pull failed for {pkgbase}");
        }
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://aur.archlinux.org/{pkgbase}.git");
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1"])
        .arg(&url)
        .arg(dir)
        .stdout(Stdio::null())
        .status()
        .with_context(|| format!("failed to run git clone for {pkgbase}"))?;
    if !status.success() {
        anyhow::bail!("git clone failed for {pkgbase}");
    }
    Ok(())
}

fn expected_artifacts(dir: &Path) -> anyhow::Result<Vec<String>> {
    let output = std::process::Command::new("makepkg")
        .arg("--packagelist")
        .current_dir(dir)
        .output()
        .context("failed to run makepkg --packagelist")?;
    if !output.status.success() {
        anyhow::bail!("makepkg --packagelist exited {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let basename = Path::new(line.trim())
                .file_name()?
                .to_string_lossy()
                .into_owned();
            (!basename.is_empty()).then_some(basename)
        })
        .collect())
}

fn run_makepkg_streaming<S: InstallSink + ?Sized>(
    dir: &Path,
    no_check: bool,
    package: &str,
    sink: &mut S,
) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("makepkg");
    cmd.args(["--noconfirm", "-f"]);
    if no_check {
        cmd.arg("--nocheck");
    }
    cmd.current_dir(dir)
        .env("PKGDEST", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("failed to spawn makepkg")?;
    if let Some(stdout) = child.stdout.take() {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(text) => sink.event(InstallEvent::BuildOutput {
                    package: package.to_string(),
                    line: text,
                }),
                Err(_) => break,
            }
        }
    }
    let status = child.wait().context("makepkg did not complete")?;
    if !status.success() {
        anyhow::bail!(
            "makepkg failed for {package} (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

pub(crate) fn spawn_install_child(targets: &[String], as_deps: bool) -> anyhow::Result<Child> {
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = crate::cli::escalation_command(&exe.to_string_lossy());
    cmd.arg("install").arg("--json");
    if as_deps {
        cmd.arg("--asdeps");
    }
    for target in targets {
        cmd.arg(target);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn().context("failed to spawn install child")
}

fn run_install_child<S: InstallSink + ?Sized>(
    targets: &[String],
    as_deps: bool,
    sink: &mut S,
) -> anyhow::Result<()> {
    let mut child = spawn_install_child(targets, as_deps)?;
    let stdout = child.stdout.take().expect("piped stdout");
    crate::events::read_event_stream(std::io::BufReader::new(stdout), sink);
    let status = child.wait().context("install child did not complete")?;
    if !status.success() {
        anyhow::bail!(
            "privileged install of [{}] failed (exit {})",
            targets.join(", "),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_pkgbase_table() {
        let valid = ["google-chrome", "a.b+c-d", "1", "A", "abc-def_ghi+jkl.mno"];
        let invalid = ["", "../x", "-r", "a/b", "a b", "--global", ".hidden", "a:b"];
        for input in valid {
            assert!(
                is_valid_pkgbase(input).is_some(),
                "is_valid_pkgbase({input:?})"
            );
        }
        for input in invalid {
            assert!(
                is_valid_pkgbase(input).is_none(),
                "is_valid_pkgbase({input:?})"
            );
        }
    }
}
