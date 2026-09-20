mod pty;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context as _;

use crate::aur::AurInfo;
use crate::cli::prompts::CliAsk;
use crate::dispatch::exec::{ChildOutcome, StreamItem};
use crate::dispatch::protocol::Decider;
use crate::events::{InstallEvent, InstallSink, PkgbuildReviewEntry};
use crate::pkgbuild::PkgbuildInfo;
use crate::resolve::{Decisions, Plan};

#[derive(Clone, Copy)]
struct AurBuildConfig {
    no_check: bool,
    as_deps: bool,
    reinstall: bool,
    tty: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BuildDecision {
    Proceed,
    Review,
    Abort,
}

pub struct BuildParams<'a> {
    pub targets: &'a [String],
    pub files: &'a [String],
    pub no_check: bool,
    pub as_deps: bool,
    pub reinstall: bool,
    pub approvals: Option<&'a str>,
    pub tty: bool,
}

pub fn run_build<S: InstallSink + ?Sized>(
    params: BuildParams<'_>,
    sink: &mut S,
    decider: &dyn Decider,
) -> anyhow::Result<()> {
    let BuildParams {
        targets,
        files,
        no_check,
        as_deps,
        reinstall,
        approvals,
        tty,
    } = params;
    let (alpm, plan) = resolve_and_report(targets, no_check, sink, tty)?;

    if plan.aur_builds().next().is_some() && crate::cli::privs::is_root() {
        anyhow::bail!("can't install AUR package as root");
    }
    crate::resolve::check_plan_gates(&plan)?;

    crate::cli::prompts::announce_conflict_calculation();
    if !plan.conflicts.is_empty() {
        crate::cli::prompts::print_conflicts(&plan.conflicts);
        crate::cli::prompts::confirm_conflict_warning(!tty);
        if !decider.confirm_conflicts(&plan.conflicts) {
            if tty {
                anyhow::bail!("build cancelled by user");
            }
            anyhow::bail!("can not install conflicting packages with --noconfirm");
        }
    }

    let decision = if plan.bases.is_empty() {
        BuildDecision::Proceed
    } else {
        let decision = decider.confirm_build(&plan);
        if matches!(decision, BuildDecision::Abort) {
            anyhow::bail!("build cancelled by user");
        }
        decision
    };

    let pkgbuilds = crate::pkgbuild::collect_for_review(&plan, sink)?;
    review_if_requested(decision, &pkgbuilds, sink, decider)?;

    let arch = alpm.architectures().first();
    let spine_confirmed = !plan.bases.is_empty();
    let repo_config = AurBuildConfig {
        no_check,
        as_deps,
        reinstall,
        tty,
    };
    install_repo_packages(&plan, files, repo_config, approvals, spine_confirmed, sink)?;

    if let Some(label) = plan.bases.iter().find_map(|base| match base {
        crate::resolve::Base::Pkgbuild { repo, base, .. } => Some(format!("{repo}/{base}")),
        _ => None,
    }) {
        anyhow::bail!("pkgbuild repo builds are not supported: {label}");
    }
    for (pkgbase, members) in plan.aur_builds() {
        let Some(first) = members.first() else {
            anyhow::bail!("resolution produced an empty package base: {pkgbase}");
        };
        let dir = clone_dir(pkgbase)?;
        let as_deps = as_deps || members.iter().all(|member| !member.target);
        let explicit = members.iter().any(|member| member.target);
        let info = AurInfo {
            name: first.name.clone(),
            package_base: pkgbase.to_string(),
            version: first.version.clone(),
            ..Default::default()
        };
        let config = AurBuildConfig {
            no_check,
            as_deps,
            reinstall: reinstall && explicit,
            tty,
        };
        build_and_install_aur(&info, &dir, config, approvals, arch, sink)?;
    }

    Ok(())
}

fn partition_repo_targets(plan: &Plan, files: &[String]) -> (Vec<String>, Vec<String>) {
    let mut explicit = files.to_vec();
    let mut deps = Vec::new();
    for row in &plan.repo_installs {
        let target = if row.db.is_empty() {
            row.name.clone()
        } else {
            format!("{}/{}", row.db, row.name)
        };
        if row.target {
            explicit.push(target);
        } else {
            deps.push(target);
        }
    }
    (explicit, deps)
}

fn install_repo_packages<S: InstallSink + ?Sized>(
    plan: &Plan,
    files: &[String],
    config: AurBuildConfig,
    approvals: Option<&str>,
    preconfirmed: bool,
    sink: &mut S,
) -> anyhow::Result<()> {
    let (explicit, deps) = partition_repo_targets(plan, files);
    if !explicit.is_empty() {
        run_install_child(
            &explicit,
            config.as_deps,
            config.reinstall,
            sink,
            approvals,
            preconfirmed,
            config.tty,
        )?;
    }
    if !deps.is_empty() {
        run_install_child(
            &deps,
            true,
            false,
            sink,
            approvals,
            preconfirmed,
            config.tty,
        )?;
    }
    Ok(())
}

fn resolve_and_report<S: InstallSink + ?Sized>(
    targets: &[String],
    no_check: bool,
    sink: &mut S,
    tty: bool,
) -> anyhow::Result<(alpm::Alpm, Plan)> {
    for target in targets {
        sink.event(InstallEvent::ResolvingAurDependencies {
            target: target.to_string(),
        });
    }

    let alpm = crate::pacman::handle()?;
    let decisions = if tty {
        Decisions::Ask(Box::new(CliAsk))
    } else {
        Decisions::Default
    };
    let plan = crate::resolve::resolve_plan_raw(targets, no_check, decisions)?;

    let repo_deps = plan.repo_installs.len();
    let aur_packages = plan.all_members().count();
    for row in &plan.repo_installs {
        sink.event(InstallEvent::AurDepResolved {
            package: row.name.clone(),
            repo: Some(row.db.clone()),
            version: Some(row.version.clone()),
        });
    }
    for member in plan.all_members() {
        sink.event(InstallEvent::AurDepResolved {
            package: member.name.clone(),
            repo: None,
            version: Some(member.version.clone()),
        });
    }
    sink.event(InstallEvent::ResolutionComplete {
        aur_packages,
        repo_deps,
    });

    Ok((alpm, plan))
}

fn review_if_requested<S: InstallSink + ?Sized>(
    decision: BuildDecision,
    pkgbuilds: &[PkgbuildInfo],
    sink: &mut S,
    decider: &dyn Decider,
) -> anyhow::Result<()> {
    if !matches!(decision, BuildDecision::Review) {
        return Ok(());
    }
    let to_review: Vec<PkgbuildInfo> = pkgbuilds
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
        if !decider.review_pkgbuilds(&to_review) {
            anyhow::bail!("PKGBUILD review rejected by user");
        }
        for pb in &to_review {
            crate::pkgbuild::mark_seen(&pb.dir)?;
        }
        sink.event(InstallEvent::PkgbuildReviewAccepted {
            packages: to_review.iter().map(|p| p.name.clone()).collect(),
        });
    }
    Ok(())
}

fn resolved_version(expected: &[String], package: &str) -> Option<String> {
    let parsed: Vec<(String, String)> = expected
        .iter()
        .filter_map(|basename| parse_package_filename(basename))
        .collect();
    parsed
        .iter()
        .find(|(pkgname, _)| pkgname.as_str() == package)
        .or_else(|| parsed.first())
        .map(|(_, version)| version.clone())
}

fn build_and_install_aur<S: InstallSink + ?Sized>(
    info: &AurInfo,
    dir: &Path,
    config: AurBuildConfig,
    approvals: Option<&str>,
    arch: Option<&str>,
    sink: &mut S,
) -> anyhow::Result<()> {
    sink.event(InstallEvent::BuildStarted {
        package: info.name.clone(),
    });
    run_makepkg_streaming(dir, config.no_check, &info.name, sink)?;

    let expected = expected_artifacts(dir)
        .with_context(|| format!("failed to enumerate artifacts for {}", info.name))?;
    let artifacts = collect_artifacts(dir, &expected)?;
    let version = resolved_version(&expected, &info.name);
    sink.event(InstallEvent::BuildCompleted {
        package: info.name.clone(),
        artifacts: artifacts.clone(),
        version,
    });

    if let Some(arch) = arch
        && let Err(e) = crate::devel::refresh_baseline(dir, arch)
    {
        eprintln!(
            "warning: devel baseline refresh failed for {}: {e:#}",
            info.package_base
        );
    }

    run_install_child(
        &artifacts,
        config.as_deps,
        config.reinstall,
        sink,
        approvals,
        true,
        config.tty,
    )?;
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

pub fn clone_dir(pkgbase: &str) -> anyhow::Result<PathBuf> {
    if is_valid_pkgbase(pkgbase).is_none() {
        anyhow::bail!("invalid pkgbase from AUR: {pkgbase:?}");
    }
    Ok(crate::utils::cache_root()?.join("aur").join(pkgbase))
}

pub fn git_clone_or_pull(dir: &Path, pkgbase: &str) -> anyhow::Result<()> {
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
    let mut cmd = makepkg_command(dir, no_check);
    let mut session = pty::PtySession::spawn(&mut cmd)?;
    let package = package.to_string();
    while let Some(line) = session.next_line()? {
        sink.event(InstallEvent::BuildOutput {
            package: package.clone(),
            line,
        });
    }
    let status = session.wait()?;
    if !status.success() {
        anyhow::bail!(
            "makepkg failed for {package} (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn makepkg_command(dir: &Path, no_check: bool) -> std::process::Command {
    let mut cmd = std::process::Command::new("makepkg");
    cmd.args(["--noconfirm", "-f"]);
    if no_check {
        cmd.arg("--nocheck");
    }
    cmd.current_dir(dir)
        .env("PKGDEST", dir)
        .stdin(Stdio::null());
    cmd
}

fn run_install_child<S: InstallSink + ?Sized>(
    targets: &[String],
    as_deps: bool,
    reinstall: bool,
    sink: &mut S,
    approvals: Option<&str>,
    preconfirmed: bool,
    tty: bool,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;
    let sealed = approvals
        .map(|payload| crate::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
        .transpose()
        .context("failed to write approvals file")?;
    let operation = crate::dispatch::operation::PrivilegedOperation::Install {
        targets: targets.to_vec(),
        as_deps,
        reinstall,
        preconfirmed,
        approvals: sealed,
    };
    let mut stream = operation.dispatch(tty);
    while let Some(item) = futures::executor::block_on(stream.next()) {
        match item {
            StreamItem::Event(event) => sink.event(event),
            StreamItem::Done(ChildOutcome::Success) => return Ok(()),
            StreamItem::Done(outcome) => {
                anyhow::bail!(
                    "privileged install of [{}] failed: {}",
                    targets.join(", "),
                    outcome.reason()
                );
            }
        }
    }
    anyhow::bail!(
        "privileged install of [{}] failed: stream ended",
        targets.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::RepoInstall;

    #[test]
    fn partition_repo_targets_pins_explicit_and_deps() {
        let plan = Plan {
            repo_installs: vec![
                RepoInstall {
                    name: "neovim".to_string(),
                    db: "extra".to_string(),
                    target: true,
                    ..Default::default()
                },
                RepoInstall {
                    name: "bare".to_string(),
                    target: true,
                    ..Default::default()
                },
                RepoInstall {
                    name: "libtermkey".to_string(),
                    db: "extra".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let files = vec!["/tmp/foo-1.0-1-x86_64.pkg.tar.zst".to_string()];
        let (explicit, deps) = partition_repo_targets(&plan, &files);
        assert_eq!(
            explicit,
            vec![
                files[0].clone(),
                "extra/neovim".to_string(),
                "bare".to_string()
            ]
        );
        assert_eq!(deps, vec!["extra/libtermkey".to_string()]);
    }

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

    #[test]
    fn resolved_version_prefers_exact_match_over_first() {
        let expected = vec![
            "bar-1.0-1-x86_64.pkg.tar.zst".to_string(),
            "foo-2.5-1-x86_64.pkg.tar.zst".to_string(),
        ];
        let version = resolved_version(&expected, "foo").expect("exact name match should be found");
        assert_eq!(version, "2.5-1");
    }

    #[test]
    fn resolved_version_falls_back_to_first_when_no_exact_match() {
        let expected = vec![
            "bar-1.0-1-x86_64.pkg.tar.zst".to_string(),
            "baz-2.0-1-any.pkg.tar.zst".to_string(),
        ];
        let version =
            resolved_version(&expected, "missing").expect("first parsed entry used as fallback");
        assert_eq!(version, "1.0-1");
    }

    #[test]
    fn resolved_version_skips_unparseable_basenames() {
        let expected = vec![
            "too-few-segments.pkg.tar.zst".to_string(),
            "foo-2.5-1-x86_64.pkg.tar.zst".to_string(),
        ];
        let version = resolved_version(&expected, "foo").expect("match after skipping unparseable");
        assert_eq!(version, "2.5-1");
    }
}
