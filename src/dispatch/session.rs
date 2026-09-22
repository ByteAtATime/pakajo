use futures::SinkExt as _;
use futures::StreamExt as _;

use crate::dispatch::exec::{ChildOutcome, StreamItem, send_done};
use crate::dispatch::operation::{BuildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;

pub struct PhasePlan {
    pub privileged: Option<PrivilegedOperation>,
    pub aur_targets: Vec<String>,
    pub files: Vec<String>,
    pub as_deps: bool,
    pub reinstall: bool,
    pub no_check: bool,
    pub repo_verb: &'static str,
    pub approvals_payload: Option<String>,
    pub decider: Box<dyn Decider + Send>,
    pub tty: bool,
}

enum ForwardEnd {
    Done(ChildOutcome),
    InnerEnded,
    ReceiverGone,
}

pub fn run_phases(plan: PhasePlan, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    if plan.privileged.is_none() && plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Failed("no install targets".to_string()));
        return;
    }
    let mut privileged_ran = false;
    if let Some(operation) = plan.privileged {
        privileged_ran = true;
        match forward(operation.dispatch(plan.tty), tx) {
            ForwardEnd::Done(ChildOutcome::Success) => {}
            ForwardEnd::Done(outcome) => {
                send_done(tx, outcome);
                return;
            }
            ForwardEnd::InnerEnded => {
                send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
                return;
            }
            ForwardEnd::ReceiverGone => return,
        }
    }
    if plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Success);
        return;
    }
    let operation = BuildOperation {
        targets: plan.aur_targets,
        files: plan.files,
        as_deps: plan.as_deps,
        reinstall: plan.reinstall,
        no_check: plan.no_check,
    };
    match forward(
        operation.dispatch(plan.decider, plan.approvals_payload, plan.tty),
        tx,
    ) {
        ForwardEnd::Done(ChildOutcome::Success) => send_done(tx, ChildOutcome::Success),
        ForwardEnd::Done(outcome) => {
            if privileged_ran {
                eprintln!(
                    "warning: repo packages {}; AUR phase failed: {}",
                    plan.repo_verb,
                    outcome.reason()
                );
            }
            send_done(tx, outcome);
        }
        ForwardEnd::InnerEnded => {
            if privileged_ran {
                eprintln!(
                    "warning: repo packages {}; AUR phase failed: stream ended",
                    plan.repo_verb
                );
            }
            send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
        }
        ForwardEnd::ReceiverGone => {}
    }
}

fn forward(
    mut inner: crate::dispatch::exec::DispatchStream,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> ForwardEnd {
    while let Some(item) = futures::executor::block_on(inner.next()) {
        match item {
            StreamItem::Done(outcome) => return ForwardEnd::Done(outcome),
            item => {
                if futures::executor::block_on(tx.send(item)).is_err() {
                    return ForwardEnd::ReceiverGone;
                }
            }
        }
    }
    ForwardEnd::InnerEnded
}
