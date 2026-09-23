use anyhow::Context as _;

use crate::aur::AurInfo;
use crate::build::{BuildDecision, build_base};
use crate::cli::prompts::CliAsk;
use crate::dispatch::exec::ChildOutcome;
use crate::dispatch::protocol::Decider;
use crate::events::{InstallEvent, InstallSink, PkgbuildReviewEntry};
use crate::pkgbuild::PkgbuildInfo;
use crate::resolve::{Decisions, Member, Plan};

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
    install_repo_packages(&plan, &params, !plan.bases.is_empty(), sink)?;
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
    let (explicit, dep_names) = explicit_and_deps(members);
    run_install_child(
        &built.artifacts,
        params,
        params.reinstall && explicit,
        true,
        &dep_names,
        sink,
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

fn repo_child_inputs(plan: &Plan, files: &[String]) -> (Vec<String>, Vec<String>) {
    let mut targets = files.to_vec();
    let mut deps = Vec::new();
    for row in &plan.repo_installs {
        if row.db.is_empty() {
            targets.push(row.name.clone());
        } else {
            targets.push(format!("{}/{}", row.db, row.name));
        }
        if !row.target {
            deps.push(row.name.clone());
        }
    }
    (targets, deps)
}

fn install_repo_packages<S: InstallSink + ?Sized>(
    plan: &Plan,
    params: &BuildParams<'_>,
    preconfirmed: bool,
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
        preconfirmed,
        &deps,
        sink,
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
) -> anyhow::Result<crate::dispatch::approvals::ApprovalsFile> {
    let payload = approvals
        .map(|seal| {
            crate::dispatch::seal::decode_seal(seal).context("failed to decode approvals seal")
        })
        .transpose()?;
    let composed = crate::question::approvals::seal_with_deps(payload.as_ref(), dep_names);
    let encoded = crate::dispatch::seal::encode_seal(&composed).context("failed to encode seal")?;
    crate::dispatch::approvals::ApprovalsFile::write(encoded.as_bytes())
        .context("failed to write approvals file")
}

fn run_install_child<S: InstallSink + ?Sized>(
    targets: &[String],
    params: &BuildParams<'_>,
    reinstall: bool,
    preconfirmed: bool,
    dep_names: &[String],
    sink: &mut S,
) -> anyhow::Result<()> {
    let sealed = compose_child_seal(params.approvals, dep_names)?;
    let operation = crate::dispatch::operation::PrivilegedOperation::Install {
        targets: targets.to_vec(),
        as_deps: params.as_deps,
        reinstall,
        preconfirmed,
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
