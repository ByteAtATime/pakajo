use std::collections::HashSet;
use std::path::Path;

use anyhow::Context as _;

use crate::aur::AurInfo;
use crate::build::{BuildDecision, build_base};
use crate::cli::prompts::CliAsk;
use crate::dispatch::exec::ChildOutcome;
use crate::dispatch::protocol::Decider;
use crate::events::{InstallEvent, InstallSink, PkgbuildReviewEntry};
use crate::pkgbuild::PkgbuildInfo;
use crate::resolve::{Decisions, Member, Plan, RepoInstall};

pub struct BuildParams<'a> {
    pub targets: &'a [String],
    pub files: &'a [String],
    pub no_check: bool,
    pub as_deps: bool,
    pub reinstall: bool,
    pub approvals: Option<&'a str>,
    pub tty: bool,
    pub interactive: bool,
}

pub fn install_aur<S: InstallSink + ?Sized>(
    params: BuildParams<'_>,
    sink: &mut S,
    decider: &dyn Decider,
) -> anyhow::Result<()> {
    let (alpm, plan) = resolve_and_report(params.targets, params.no_check, sink, params.tty)?;
    reject_root_build(&plan)?;
    crate::resolve::check_plan_gates(&plan)?;
    confirm_conflicts(&plan, decider, params.tty)?;
    let decision = confirm_build(&plan, decider)?;
    let pkgbuilds = crate::pkgbuild::collect_for_review(&plan, sink)?;
    review_if_requested(decision, &pkgbuilds, sink, decider)?;
    let arch = alpm.architectures().first();
    install_repo_packages(&plan, &params, sink)?;
    reject_pkgbuild_bases(&plan)?;
    for (pkgbase, members) in plan.aur_builds() {
        install_aur_base(pkgbase, members, &params, arch, sink)?;
    }
    Ok(())
}

fn reject_root_build(plan: &Plan) -> anyhow::Result<()> {
    if plan.aur_builds().next().is_some() && crate::cli::privs::is_root() {
        anyhow::bail!("can't install AUR package as root");
    }
    Ok(())
}

fn confirm_conflicts(plan: &Plan, decider: &dyn Decider, tty: bool) -> anyhow::Result<()> {
    crate::cli::prompts::announce_conflict_calculation();
    if plan.conflicts.is_empty() {
        return Ok(());
    }
    crate::cli::prompts::print_conflicts(&plan.conflicts);
    crate::cli::prompts::confirm_conflict_warning(!tty);
    if decider.confirm_conflicts(&plan.conflicts) {
        return Ok(());
    }
    if tty {
        anyhow::bail!("build cancelled by user");
    }
    anyhow::bail!("can not install conflicting packages with --noconfirm");
}

fn confirm_build(plan: &Plan, decider: &dyn Decider) -> anyhow::Result<BuildDecision> {
    if plan.bases.is_empty() {
        return Ok(BuildDecision::Proceed);
    }
    let decision = decider.confirm_build(plan);
    if matches!(decision, BuildDecision::Abort) {
        anyhow::bail!("build cancelled by user");
    }
    Ok(decision)
}

fn reject_pkgbuild_bases(plan: &Plan) -> anyhow::Result<()> {
    let Some(label) = plan.bases.iter().find_map(|base| match base {
        crate::resolve::Base::Pkgbuild { repo, base, .. } => Some(format!("{repo}/{base}")),
        _ => None,
    }) else {
        return Ok(());
    };
    anyhow::bail!("pkgbuild repo builds are not supported: {label}");
}

fn install_aur_base<S: InstallSink + ?Sized>(
    pkgbase: &str,
    members: &[Member],
    params: &BuildParams<'_>,
    arch: Option<&str>,
    sink: &mut S,
) -> anyhow::Result<()> {
    let Some(first) = members.first() else {
        anyhow::bail!("resolution produced an empty package base: {pkgbase}");
    };
    let dir = crate::build::clone_dir(pkgbase)?;
    let info = AurInfo {
        name: first.name.clone(),
        package_base: pkgbase.to_string(),
        version: first.version.clone(),
        ..Default::default()
    };
    sink.event(InstallEvent::BuildStarted {
        package: info.name.clone(),
    });
    let package = info.name.clone();
    let built = build_base(&dir, &info.name, params.no_check, |line| {
        sink.event(InstallEvent::BuildOutput {
            package: package.clone(),
            line,
        });
    })?;
    sink.event(InstallEvent::BuildCompleted {
        package: info.name.clone(),
        artifacts: built.artifacts.clone(),
        version: built.version,
    });
    if let Some(arch) = arch
        && let Err(error) = crate::devel::refresh_baseline(&dir, arch)
    {
        eprintln!(
            "warning: devel baseline refresh failed for {}: {error:#}",
            info.package_base
        );
    }
    let (explicit, mut dep_names) = explicit_and_deps(members);
    dep_names.extend(split_dep_names(&built.artifacts, members));
    run_install_child(
        &built.artifacts,
        params,
        params.reinstall && explicit,
        &dep_names,
        sink,
        true,
    )
}

fn explicit_and_deps(members: &[Member]) -> (bool, Vec<String>) {
    let mut explicit = false;
    let mut deps = Vec::new();
    for member in members {
        if member.target {
            explicit = true;
        } else {
            deps.push(member.name.clone());
        }
    }
    (explicit, deps)
}

fn split_dep_names(artifacts: &[String], members: &[Member]) -> Vec<String> {
    let targets: HashSet<&str> = members
        .iter()
        .filter(|member| member.target)
        .map(|member| member.name.as_str())
        .collect();
    artifacts
        .iter()
        .filter_map(|artifact| {
            let basename = Path::new(artifact)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let (name, _) = crate::build::parse_package_filename(&basename)?;
            (!targets.contains(name.as_str())).then_some(name)
        })
        .collect()
}

fn repo_target(row: &RepoInstall) -> String {
    if row.db.is_empty() {
        row.name.clone()
    } else {
        format!("{}/{}", row.db, row.name)
    }
}

fn repo_child_inputs(plan: &Plan, files: &[String]) -> (Vec<String>, Vec<String>) {
    let mut targets = files.to_vec();
    let mut deps = Vec::new();
    for row in &plan.repo_installs {
        targets.push(repo_target(row));
        if !row.target {
            deps.push(row.name.clone());
        }
    }
    (targets, deps)
}

fn install_repo_packages<S: InstallSink + ?Sized>(
    plan: &Plan,
    params: &BuildParams<'_>,
    sink: &mut S,
) -> anyhow::Result<()> {
    let (targets, deps) = repo_child_inputs(plan, params.files);
    if targets.is_empty() {
        return Ok(());
    }
    run_install_child(
        &targets,
        params,
        params.reinstall,
        &deps,
        sink,
        !plan.bases.is_empty(),
    )
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
        aur_packages: plan.all_members().count(),
        repo_deps: plan.repo_installs.len(),
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
        return Ok(());
    }
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
    Ok(())
}

fn compose_child_seal(
    approvals: Option<&str>,
    dep_names: &[String],
    confirmed: bool,
) -> anyhow::Result<crate::dispatch::approvals::ApprovalsFile> {
    let payload = approvals
        .map(|seal| {
            crate::dispatch::seal::decode_seal(seal).context("failed to decode approvals seal")
        })
        .transpose()?;
    let mut composed = crate::question::approvals::seal_with_deps(payload.as_ref(), dep_names);
    if payload.is_none() {
        composed.proceed = confirmed;
    }
    let encoded = crate::dispatch::seal::encode_seal(&composed).context("failed to encode seal")?;
    crate::dispatch::approvals::ApprovalsFile::write(encoded.as_bytes())
        .context("failed to write approvals file")
}

fn run_install_child<S: InstallSink + ?Sized>(
    targets: &[String],
    params: &BuildParams<'_>,
    reinstall: bool,
    dep_names: &[String],
    sink: &mut S,
    confirmed: bool,
) -> anyhow::Result<()> {
    let sealed = compose_child_seal(params.approvals, dep_names, confirmed)?;
    let operation = crate::dispatch::operation::PrivilegedOperation::Install {
        targets: targets.to_vec(),
        as_deps: params.as_deps,
        reinstall,
        interactive: params.interactive,
        approvals: Some(sealed),
    };
    match crate::dispatch::exec::drain_declining(operation.dispatch(params.tty), sink) {
        ChildOutcome::Success => Ok(()),
        outcome => anyhow::bail!(
            "privileged install of [{}] failed: {}",
            targets.join(", "),
            outcome.reason()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, target: bool) -> Member {
        Member {
            name: name.to_string(),
            version: "1.0-1".to_string(),
            make: false,
            target,
        }
    }

    #[test]
    fn split_dep_names_marks_only_non_target_artifacts() {
        let members = vec![member("bitbake", true)];
        let artifacts = vec![
            "/cache/bitbake-6.0-4-x86_64.pkg.tar.zst".to_string(),
            "/cache/bitbake-vim-6.0-4-x86_64.pkg.tar.zst".to_string(),
        ];
        assert_eq!(
            split_dep_names(&artifacts, &members),
            vec!["bitbake-vim".to_string()]
        );
    }

    #[test]
    fn split_dep_names_ignores_targets_and_unparseable_files() {
        let members = vec![member("foo", true), member("foo-doc", false)];
        let artifacts = vec![
            "/cache/foo-2.5-1-x86_64.pkg.tar.zst".to_string(),
            "/cache/foo-doc-2.5-1-x86_64.pkg.tar.zst".to_string(),
            "/cache/README".to_string(),
        ];
        assert_eq!(
            split_dep_names(&artifacts, &members),
            vec!["foo-doc".to_string()]
        );
    }

    fn sealed_proceed(file: &crate::dispatch::approvals::ApprovalsFile) -> bool {
        let payload = std::fs::read_to_string(file.path()).expect("seal file readable");
        crate::dispatch::seal::decode_seal(&payload)
            .expect("seal decodes")
            .proceed
    }

    fn encoded_payload(proceed: bool) -> String {
        crate::dispatch::seal::encode_seal(&crate::question::approvals::SealedApprovals {
            answers: Vec::new(),
            proceed,
            deps: Vec::new(),
        })
        .expect("payload encodes")
    }

    #[test]
    fn unconfirmed_synthesis_seals_proceed_false() {
        let sealed = compose_child_seal(None, &["dep".to_string()], false).expect("seal writes");
        assert!(!sealed_proceed(&sealed));
    }

    #[test]
    fn confirmed_synthesis_seals_proceed_true() {
        let sealed = compose_child_seal(None, &["dep".to_string()], true).expect("seal writes");
        assert!(sealed_proceed(&sealed));
    }

    #[test]
    fn user_payload_proceed_survives_synthesis() {
        for proceed in [true, false] {
            let payload = encoded_payload(proceed);
            let sealed = compose_child_seal(Some(&payload), &["dep".to_string()], true)
                .expect("seal writes");
            assert_eq!(sealed_proceed(&sealed), proceed);
            let sealed = compose_child_seal(Some(&payload), &["dep".to_string()], false)
                .expect("seal writes");
            assert_eq!(sealed_proceed(&sealed), proceed);
        }
    }
}
