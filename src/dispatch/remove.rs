use anyhow::Context as _;
use futures::SinkExt as _;
use futures::StreamExt as _;

use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::{ChildOperation, PrivilegedOperation};

pub struct RemoveRequest {
    pub targets: Vec<String>,
    pub tty: bool,
    pub json: bool,
    pub approvals: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Preview {
    pub summary: crate::events::TransactionSummary,
    pub questions: crate::question::QuestionSet,
    pub prepare_error: Option<crate::dry_run::PrepareFailure>,
    pub aur: Vec<crate::upgrade::AurUpgradeCandidate>,
    pub pkgbuild_diffs: Vec<crate::pkgbuild::PkgbuildDiff>,
}

pub fn remove(request: RemoveRequest) -> DispatchStream {
    let (tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || run_remove(request, tx));
    rx
}

pub fn preview(request: &RemoveRequest) -> anyhow::Result<Preview> {
    let mut handle = crate::pacman::handle_rootless()?;
    preview_with_handle(&mut handle, request)
}

fn preview_with_handle(
    handle: &mut alpm::Alpm,
    request: &RemoveRequest,
) -> anyhow::Result<Preview> {
    let state = crate::dry_run::attach_recorder(handle);
    let outcome = run_remove_preview(handle, request, &state);
    let _ = handle.trans_release();
    outcome
}

fn run_remove_preview(
    handle: &mut alpm::Alpm,
    request: &RemoveRequest,
    state: &std::rc::Rc<std::cell::RefCell<crate::dry_run::RecorderState>>,
) -> anyhow::Result<Preview> {
    let config = crate::pacman::config()?;
    let targets = expand_remove_groups(handle, &request.targets, request.tty);
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init remove preview transaction")?;
    for name in &targets {
        let pkg = handle
            .localdb()
            .pkg(name.as_str())
            .map_err(|_| anyhow::anyhow!("package '{name}' is not installed"))?;
        handle
            .trans_remove_pkg(pkg)
            .context("failed to queue package for removal")?;
    }
    let prepare_error = handle
        .trans_prepare()
        .err()
        .map(crate::dry_run::extract_prepare_failure);
    let mut questions = crate::dry_run::snapshot(state);
    if prepare_error.is_none() {
        let patterns = &config.hold_pkg;
        let names: Vec<String> = handle
            .trans_remove()
            .iter()
            .map(|pkg| pkg.name().to_string())
            .collect();
        questions.held = crate::holdpkg::held_packages(&names, patterns);
    }
    let summary = crate::install::build_summary(handle);
    Ok(Preview {
        summary,
        questions,
        prepare_error,
        aur: Vec::new(),
        pkgbuild_diffs: Vec::new(),
    })
}

fn seal_approvals(
    approvals: &Option<String>,
) -> anyhow::Result<Option<crate::dispatch::approvals::ApprovalsFile>> {
    approvals
        .as_deref()
        .map(|payload| crate::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
        .transpose()
}

fn run_remove(request: RemoveRequest, mut tx: futures::channel::mpsc::Sender<StreamItem>) {
    if crate::cli::privs::is_root() {
        run_root_remove(request, &mut tx);
        return;
    }
    let handle = match crate::pacman::handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let targets = expand_remove_groups(&handle, &request.targets, request.tty);
    drop(handle);
    let sealed = match seal_approvals(&request.approvals) {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let mut inner = PrivilegedOperation::Remove {
        targets,
        approvals: sealed,
    }
    .dispatch(request.tty);
    while let Some(item) = futures::executor::block_on(inner.next()) {
        if let Err(error) = futures::executor::block_on(tx.send(item)) {
            eprintln!("warning: dispatch stream closed: {error}");
            return;
        }
    }
}

fn run_root_remove(request: RemoveRequest, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    let handle = match crate::pacman::handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let targets = expand_remove_groups(&handle, &request.targets, request.tty);
    drop(handle);
    let sealed = match seal_approvals(&request.approvals) {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let operation = ChildOperation::Remove {
        targets,
        approvals_path: sealed
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        stream: request.json,
    };
    let outcome = match operation.execute() {
        Ok(()) => ChildOutcome::Success,
        Err(error) => ChildOutcome::Failed(format!("{error:#}")),
    };
    send_done(tx, outcome);
}

fn expand_remove_groups(
    handle: &alpm::Alpm,
    positionals: &[String],
    interactive: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in positionals {
        if crate::package::is_installed(handle, s) {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        if let Some(group) = crate::package::local_group(handle, s) {
            let members: Vec<String> = if interactive {
                crate::cli::prompts::select_group_members(s, std::slice::from_ref(&group))
            } else {
                group.members.iter().map(|m| m.name.clone()).collect()
            };
            for name in members {
                if seen.insert(name.clone()) {
                    out.push(name);
                }
            }
            continue;
        }
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}
