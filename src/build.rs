use std::io::Read as _;
use std::os::unix::io::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

use anyhow::Context as _;

use crate::events::{AurDepSource, InstallEvent, InstallSink};
use crate::resolve::BuildPlan;

pub enum BuildDecision {
    Proceed,
    Review,
    Abort,
}

pub fn run_build<S: InstallSink + ?Sized>(
    targets: &[String],
    no_check: bool,
    user_as_deps: bool,
    sink: &mut S,
    confirm: impl FnOnce(&BuildPlan) -> BuildDecision,
    review: impl FnOnce(&[crate::pkgbuild::PkgbuildInfo]) -> bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<()> {
    for target in targets {
        sink.event(InstallEvent::ResolvingAurDependencies {
            target: target.to_string(),
        });
    }

    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let alpm = crate::pacman::init_alpm(&config)?;
    let aur = crate::aur::AurClient::new();
    let plan = crate::resolve::resolve(&crate::resolve::AlpmDb(&alpm), &aur, targets, no_check)?;

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

    let decision = confirm(&plan);
    if matches!(decision, BuildDecision::Abort) {
        anyhow::bail!("build cancelled by user");
    }

    let pkgbuilds = crate::pkgbuild::collect_for_review(&plan, sink)?;
    if matches!(decision, BuildDecision::Review) {
        use crate::events::{InstallEvent, PkgbuildReviewEntry};
        let to_review: Vec<crate::pkgbuild::PkgbuildInfo> = pkgbuilds
            .iter()
            .filter(|p| p.needs_review)
            .cloned()
            .collect();
        if to_review.is_empty() {
            sink.event(InstallEvent::PkgbuildAllUpToDate {
                packages: pkgbuilds.iter().map(|p| p.name.clone()).collect(),
            });
        } else {
            sink.event(InstallEvent::PkgbuildReviewStarted {
                packages: to_review
                    .iter()
                    .map(|p| PkgbuildReviewEntry {
                        name: p.name.clone(),
                        pkgbase: p.pkgbase.clone(),
                        is_new: p.is_new,
                    })
                    .collect(),
            });
            if !review(&to_review) {
                anyhow::bail!("PKGBUILD review rejected by user");
            }
            for pb in &to_review {
                crate::pkgbuild::mark_seen(&pb.dir)?;
            }
            sink.event(InstallEvent::PkgbuildReviewAccepted {
                packages: to_review.iter().map(|p| p.name.clone()).collect(),
            });
        }
    }

    let total_layers = plan.layers.len();
    let arch = alpm.architectures().first();
    for (idx, layer) in plan.layers.iter().enumerate() {
        sink.event(InstallEvent::LayerBoundary {
            layer: idx,
            total: total_layers,
        });

        if !layer.repo_deps.is_empty() {
            run_install_child(&layer.repo_deps, true, sink, None)?;
        }

        for info in &layer.aur {
            let pkgbase = &info.package_base;
            let dir = clone_dir(pkgbase)?;

            sink.event(InstallEvent::BuildStarted {
                package: info.name.clone(),
            });
            run_makepkg_streaming(&dir, no_check, &info.name, sink)?;

            let expected = expected_artifacts(&dir)
                .with_context(|| format!("failed to enumerate artifacts for {}", info.name))?;
            let artifacts = collect_artifacts(&dir, &expected)?;
            let parsed: Vec<(String, String)> = expected
                .iter()
                .filter_map(|basename| parse_package_filename(basename))
                .collect();
            let version = parsed
                .iter()
                .find(|(pkgname, _)| pkgname == &info.name)
                .or_else(|| parsed.first())
                .map(|(_, version)| version.clone());
            sink.event(InstallEvent::BuildCompleted {
                package: info.name.clone(),
                artifacts: artifacts.clone(),
                version,
            });

            if let Some(arch) = arch
                && let Err(e) = crate::devel::refresh_baseline(&dir, arch) {
                    eprintln!("warning: devel baseline refresh failed for {pkgbase}: {e:#}");
                }

            let as_deps = user_as_deps || !plan.targets.iter().any(|t| t == &info.name);
            run_install_child(&artifacts, as_deps, sink, approvals_b64)?;
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

pub(crate) fn cache_root() -> anyhow::Result<PathBuf> {
    let cache = match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => PathBuf::from(xdg),
        Err(_) => {
            let home =
                std::env::var("HOME").context("no cache directory: set XDG_CACHE_HOME or HOME")?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(cache.join("pakajo"))
}

pub(crate) fn clone_dir(pkgbase: &str) -> anyhow::Result<PathBuf> {
    if is_valid_pkgbase(pkgbase).is_none() {
        anyhow::bail!("invalid pkgbase from AUR: {pkgbase:?}");
    }
    Ok(cache_root()?.join(pkgbase))
}

pub(crate) fn git_clone_or_pull(dir: &Path, pkgbase: &str) -> anyhow::Result<()> {
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

fn parse_package_filename(basename: &str) -> Option<(String, String)> {
    let split: Vec<&str> = basename.split('-').collect();
    if split.len() < 4 {
        return None;
    }
    let pkgname = split[..split.len() - 3].join("-");
    let version = split[split.len() - 3..split.len() - 1].join("-");
    Some((pkgname, version))
}

fn run_makepkg_streaming<S: InstallSink + ?Sized>(
    dir: &Path,
    no_check: bool,
    package: &str,
    sink: &mut S,
) -> anyhow::Result<()> {
    let win_size = crate::utils::terminal_winsize();
    let pty = nix::pty::openpty(&win_size, None).context("failed to open pseudoterminal")?;
    let slave_stdout = pty.slave.try_clone().context("failed to clone pty slave")?;
    let slave_stderr = pty.slave.try_clone().context("failed to clone pty slave")?;

    let mut cmd = std::process::Command::new("makepkg");
    cmd.args(["--noconfirm", "-f"]);
    if no_check {
        cmd.arg("--nocheck");
    }
    cmd.current_dir(dir)
        .env("PKGDEST", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(slave_stdout))
        .stderr(Stdio::from(slave_stderr));

    let mut child = cmd.spawn().context("failed to spawn makepkg")?;
    drop(pty.slave);

    let package = package.to_string();
    let mut master = std::fs::File::from(pty.master);
    set_nonblocking(&master).context("failed to set pty master non-blocking")?;

    let mut buf = [0u8; 4096];
    let mut pending_line: Vec<u8> = Vec::new();
    let mut child_gone = false;

    let mut emit = |line: &mut Vec<u8>| {
        let mut text = String::from_utf8_lossy(line).into_owned();
        if text.ends_with('\r') {
            text.pop();
        }
        sink.event(InstallEvent::BuildOutput {
            package: package.clone(),
            line: text,
        });
        line.clear();
    };

    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' {
                        emit(&mut pending_line);
                    } else {
                        pending_line.push(byte);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if !child_gone {
                    match child.try_wait()? {
                        None => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Some(_) => child_gone = true,
                    }
                }
                let mut pfd = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut pfd, 1, 200) };
                if ready <= 0 {
                    break;
                }
            }
            Err(e) => return Err(e).context("failed to read makepkg output"),
        }
    }

    if !pending_line.is_empty() {
        emit(&mut pending_line);
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

fn set_nonblocking(file: &std::fs::File) -> anyhow::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error()).context("fcntl F_GETFL on pty master");
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("fcntl F_SETFL on pty master");
    }
    Ok(())
}

pub(crate) fn spawn_install_child(
    targets: &[String],
    as_deps: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<Child> {
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = crate::cli::escalation_command(&exe.to_string_lossy());
    cmd.arg("install").arg("--json");
    if as_deps {
        cmd.arg("--asdeps");
    }
    if let Some(b64) = approvals_b64 {
        cmd.arg("--approvals").arg(b64);
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
    approvals_b64: Option<&str>,
) -> anyhow::Result<()> {
    let mut child = spawn_install_child(targets, as_deps, approvals_b64)?;
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

    #[test]
    fn parse_package_filename_extracts_git_pkgver() {
        let (pkgname, version) =
            parse_package_filename("cava-git-r1136.20a5997-2-x86_64.pkg.tar.zst")
                .expect("parseable vcs package basename");
        assert_eq!(pkgname, "cava-git");
        assert_eq!(version, "r1136.20a5997-2");
    }

    #[test]
    fn parse_package_filename_handles_pkgname_with_many_dashes() {
        let (pkgname, version) =
            parse_package_filename("python-tokenize-rt-5.2.0-3-any.pkg.tar.zst")
                .expect("pkgname with multiple dashes still parses");
        assert_eq!(pkgname, "python-tokenize-rt");
        assert_eq!(version, "5.2.0-3");
    }

    #[test]
    fn parse_package_filename_rejects_too_few_segments() {
        assert!(parse_package_filename("foo-1.pkg.tar.zst").is_none());
        assert!(parse_package_filename("foo-1-2").is_none());
    }
}
