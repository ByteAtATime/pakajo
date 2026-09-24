use anyhow::Context as _;
use futures::SinkExt as _;
use futures::StreamExt as _;

use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::{ChildOperation, PrivilegedOperation};
use crate::dispatch::seal::{JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing};
use crate::events::DiscardSink;

pub struct RemoveRequest {
    pub targets: Vec<String>,
    pub tty: bool,
    pub json: bool,
    pub approvals: Option<String>,
}

pub(crate) const NON_INTERACTIVE_SEAL_REQUIRED: &str =
    "non-interactive remove requires sealed approvals";

pub fn remove(request: RemoveRequest) -> DispatchStream {
    let (mut tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || {
        if json_seal_missing(request.json, request.approvals.as_deref()) {
            send_done(
                &mut tx,
                ChildOutcome::Failed(JSON_SEAL_REQUIRED.to_string()),
            );
            return;
        }
        if non_interactive_seal_missing(request.json, request.tty, request.approvals.as_deref()) {
            send_done(
                &mut tx,
                ChildOutcome::Failed(NON_INTERACTIVE_SEAL_REQUIRED.to_string()),
            );
            return;
        }
        if request.targets.is_empty() {
            send_done(
                &mut tx,
                ChildOutcome::Failed("no remove targets".to_string()),
            );
            return;
        }
        run_remove(request, tx);
    });
    rx
}

pub(crate) fn run_remove_preview(
    handle: &mut alpm::Alpm,
    targets: &[String],
    holds: &[String],
    source: Box<dyn crate::question::source::AnswerSource>,
) -> anyhow::Result<crate::question::review::Review> {
    let spec = crate::tx::driver::RunSpec {
        kind: crate::tx::driver::RunKind::Remove(crate::tx::driver::RemoveSpec {
            flags: alpm::TransFlag::NONE,
            holds: holds.to_vec(),
        }),
        targets: targets.to_vec(),
        stub_targets: Vec::new(),
        explore: true,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    };
    let outcome = crate::tx::driver::run(handle, &spec, source, Box::new(DiscardSink))?;
    outcome.review.context("remove preview produced no review")
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
    let targets = request.targets.clone();
    let sealed = match seal_approvals(&request.approvals) {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let mut inner = PrivilegedOperation::Remove {
        targets,
        interactive: request.tty,
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
    let targets = request.targets.clone();
    let sealed = match seal_approvals(&request.approvals) {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let operation = ChildOperation::Remove {
        targets,
        interactive: request.tty && !request.json,
        approvals_path: sealed
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        stream: request.json,
    };
    let outcome = match operation.execute() {
        Ok(code) => crate::dispatch::exec::map_exit_code(
            crate::dispatch::exec::ChildKind::Remove,
            code,
            "direct",
        ),
        Err(error) => ChildOutcome::Failed(format!("{error:#}")),
    };
    send_done(tx, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::seal::{
        JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing,
    };

    #[test]
    fn missing_seals_are_rejected_with_exact_messages() {
        assert!(json_seal_missing(true, None));
        assert!(!json_seal_missing(true, Some("{}")));
        assert!(!json_seal_missing(false, None));
        assert_eq!(JSON_SEAL_REQUIRED, "--json requires sealed approvals");
        assert!(non_interactive_seal_missing(false, false, None));
        assert!(!non_interactive_seal_missing(false, true, None));
        assert!(!non_interactive_seal_missing(true, false, None));
        assert!(!non_interactive_seal_missing(false, false, Some("{}")));
        assert_eq!(
            NON_INTERACTIVE_SEAL_REQUIRED,
            "non-interactive remove requires sealed approvals"
        );
    }

    #[test]
    fn empty_targets_fail_closed_without_spawning() {
        let request = RemoveRequest {
            targets: Vec::new(),
            tty: true,
            json: false,
            approvals: None,
        };
        let outcome = crate::dispatch::exec::drain_declining(remove(request), &mut DiscardSink);
        assert!(matches!(outcome, ChildOutcome::Failed(message) if message == "no remove targets"));
    }
}
