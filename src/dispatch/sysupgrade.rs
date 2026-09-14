use anyhow::Context as _;
use futures::SinkExt as _;

use crate::dispatch::approvals::ApprovalsFile;
use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::{ChildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;
use crate::dispatch::remove::Preview;
use crate::dispatch::session::{PhasePlan, run_phases};
use crate::events::InstallEvent;

pub struct SysupgradeRequest {
    pub no_refresh: bool,
    pub repo_only: bool,
    pub ignores: Vec<String>,
    pub decider: Box<dyn Decider + Send>,
    pub aur_targets: Option<Vec<String>>,
    pub fingerprint: Option<ApprovalsFile>,
    pub approvals: Option<String>,
    pub tty: bool,
    pub json: bool,
}

pub fn sysupgrade(request: SysupgradeRequest) -> DispatchStream {
    let (tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || run_sysupgrade(request, tx));
    rx
}

pub struct SysupgradePreviewRequest {
    pub no_refresh: bool,
    pub ignores: Vec<String>,
}

pub fn sysupgrade_preview(request: &SysupgradePreviewRequest) -> anyhow::Result<Preview> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm_rootless(&config)?;
    if !request.no_refresh {
        crate::pacman::refresh_sync_dbs_rootless(&mut handle)?;
    }
    crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores);
    let aur = match detect_aur_upgrades(&handle) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!(
                "[pakajo] aur upgrade check failed, sysupgrade preview shows repo only: {error:#}"
            );
            Vec::new()
        }
    };
    let dry = crate::dry_run::dry_sysupgrade(&mut handle)?;
    let aur_names: Vec<String> = aur.iter().map(|candidate| candidate.name.clone()).collect();
    let pkgbuild_diffs = if aur_names.is_empty() {
        Vec::new()
    } else {
        match crate::pkgbuild::prepare_pkgbuild_diffs(&aur_names) {
            Ok(diffs) => diffs,
            Err(error) => {
                eprintln!("[pakajo] pkgbuild diff computation failed: {error:#}");
                Vec::new()
            }
        }
    };
    Ok(Preview {
        summary: dry.summary,
        questions: dry.questions,
        prepare_error: dry.prepare_error,
        aur,
        pkgbuild_diffs,
    })
}

fn detect_aur_upgrades(
    handle: &alpm::Alpm,
) -> anyhow::Result<Vec<crate::upgrade::AurUpgradeCandidate>> {
    let aur = crate::aur::AurClient::new();
    let (candidates, _) =
        crate::upgrade::compute_aur_upgrades(handle, &aur, crate::upgrade::DevelSource::Live)?;
    let ignored: std::collections::HashSet<&str> = handle.ignorepkgs().iter().collect();
    Ok(candidates
        .into_iter()
        .filter(|candidate| !ignored.contains(candidate.name.as_str()))
        .collect())
}

fn run_sysupgrade(request: SysupgradeRequest, mut tx: futures::channel::mpsc::Sender<StreamItem>) {
    if crate::cli::privs::is_root() {
        run_root_sysupgrade(request, &mut tx);
        return;
    }
    let Some(aur_targets) = resolve_aur_targets(&request, &mut tx) else {
        return;
    };
    let sealed = match request
        .approvals
        .as_deref()
        .map(|payload| ApprovalsFile::write(payload.as_bytes()))
        .transpose()
    {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    run_phases(
        PhasePlan {
            privileged: Some(PrivilegedOperation::UpgradeRepo {
                no_refresh: request.no_refresh,
                ignores: request.ignores,
                fingerprint: request.fingerprint,
                approvals: sealed,
            }),
            aur_targets,
            as_deps: false,
            repo_verb: "upgraded",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
        },
        &mut tx,
    );
}

fn run_root_sysupgrade(
    request: SysupgradeRequest,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) {
    let Some(aur_targets) = resolve_aur_targets(&request, tx) else {
        return;
    };
    let sealed = match request
        .approvals
        .as_deref()
        .map(|payload| ApprovalsFile::write(payload.as_bytes()))
        .transpose()
    {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let operation = ChildOperation::UpgradeRepo {
        no_refresh: request.no_refresh,
        ignores: request.ignores,
        fingerprint_path: request
            .fingerprint
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        approvals_path: sealed
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        stream: request.json,
    };
    let outcome = match operation.execute() {
        Ok(()) => ChildOutcome::Success,
        Err(error) => ChildOutcome::Failed(format!("{error:#}")),
    };
    if !matches!(outcome, ChildOutcome::Success) {
        send_done(tx, outcome);
        return;
    }
    if request.repo_only || aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Success);
        return;
    }
    run_phases(
        PhasePlan {
            privileged: None,
            aur_targets,
            as_deps: false,
            repo_verb: "upgraded",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
        },
        tx,
    );
}

fn resolve_aur_targets(
    request: &SysupgradeRequest,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<Vec<String>> {
    if request.repo_only {
        return Some(Vec::new());
    }
    if let Some(targets) = &request.aur_targets {
        return Some(targets.clone());
    }
    let mut handle = match crate::cli::alpm_handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return None;
        }
    };
    match pacmanconf::Config::new() {
        Ok(config) => crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores),
        Err(error) => {
            eprintln!("warning: failed to read pacman config, skipping ignore filters: {error:#}")
        }
    }
    let mut candidates = match detect_aur_upgrades(&handle) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!("warning: AUR upgrade detection failed: {error:#}");
            Vec::new()
        }
    };
    drop(handle);
    candidates.retain(|candidate| !request.ignores.contains(&candidate.name));
    let targets: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.name.clone())
        .collect();
    if futures::executor::block_on(tx.send(StreamItem::Event(
        InstallEvent::SysupgradeAurCandidates { candidates },
    )))
    .is_err()
    {
        return None;
    }
    Some(targets)
}
