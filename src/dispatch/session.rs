use futures::SinkExt as _;
use futures::StreamExt as _;

use crate::dispatch::approvals::ApprovalsFile;
use crate::dispatch::exec::{ChildOutcome, StreamItem};
use crate::dispatch::operation::{BuildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;

pub struct PhasePlan {
    pub repo_targets: Vec<String>,
    pub aur_targets: Vec<String>,
    pub as_deps: bool,
    pub approvals: Option<ApprovalsFile>,
    pub approvals_payload: Option<String>,
    pub decider: Box<dyn Decider + Send>,
    pub tty: bool,
}

pub fn run_phases(plan: PhasePlan, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    if plan.repo_targets.is_empty() && plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Failed("no install targets".to_string()));
        return;
    }
    let mut repo_ran = false;
    if !plan.repo_targets.is_empty() {
        repo_ran = true;
        let operation = PrivilegedOperation::Install {
            targets: plan.repo_targets,
            as_deps: plan.as_deps,
            approvals: plan.approvals,
        };
        match forward(operation.dispatch(plan.tty), tx) {
            Some(ChildOutcome::Success) => {}
            Some(outcome) => {
                send_done(tx, outcome);
                return;
            }
            None => {
                send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
                return;
            }
        }
    }
    if plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Success);
        return;
    }
    let operation = BuildOperation {
        targets: plan.aur_targets,
        as_deps: plan.as_deps,
        no_check: false,
    };
    match forward(
        operation.dispatch(plan.decider, plan.approvals_payload, plan.tty),
        tx,
    ) {
        Some(ChildOutcome::Success) => send_done(tx, ChildOutcome::Success),
        Some(outcome) => {
            if repo_ran {
                eprintln!(
                    "warning: repo packages installed; AUR phase failed: {}",
                    outcome.reason()
                );
            }
            send_done(tx, outcome);
        }
        None => {
            if repo_ran {
                eprintln!("warning: repo packages installed; AUR phase failed: stream ended");
            }
            send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
        }
    }
}

fn forward(
    mut inner: crate::dispatch::exec::DispatchStream,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<ChildOutcome> {
    while let Some(item) = futures::executor::block_on(inner.next()) {
        match item {
            StreamItem::Event(event) => {
                if futures::executor::block_on(tx.send(StreamItem::Event(event))).is_err() {
                    eprintln!("warning: dispatch stream closed");
                    return None;
                }
            }
            StreamItem::Done(outcome) => return Some(outcome),
        }
    }
    None
}

fn send_done(tx: &mut futures::channel::mpsc::Sender<StreamItem>, outcome: ChildOutcome) {
    if let Err(error) = futures::executor::block_on(tx.send(StreamItem::Done(outcome))) {
        eprintln!("warning: dispatch stream closed: {error}");
    }
}
