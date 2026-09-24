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
    pub interactive: bool,
}

enum ForwardEnd {
    Done(ChildOutcome),
    InnerEnded,
    ReceiverGone,
}

pub enum RepoPhaseDecision {
    ContinueToAur,
    Finish(ChildOutcome),
}

pub fn repo_phase_decision(outcome: &ChildOutcome, aur_pending: bool) -> RepoPhaseDecision {
    match outcome {
        ChildOutcome::Success | ChildOutcome::Stopped { idle: true } if aur_pending => {
            RepoPhaseDecision::ContinueToAur
        }
        _ => RepoPhaseDecision::Finish(outcome.clone()),
    }
}

pub fn run_phases(plan: PhasePlan, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    if plan.privileged.is_none() && plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Failed("no install targets".to_string()));
        return;
    }
    let mut privileged_ran = false;
    if let Some(operation) = plan.privileged {
        privileged_ran = true;
        let outcome = match forward(operation.dispatch(plan.tty), tx) {
            ForwardEnd::Done(outcome) => outcome,
            ForwardEnd::InnerEnded => {
                send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
                return;
            }
            ForwardEnd::ReceiverGone => return,
        };
        match repo_phase_decision(&outcome, !plan.aur_targets.is_empty()) {
            RepoPhaseDecision::ContinueToAur => {}
            RepoPhaseDecision::Finish(outcome) => {
                send_done(tx, outcome);
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
        files: plan.files,
        as_deps: plan.as_deps,
        reinstall: plan.reinstall,
        no_check: plan.no_check,
        interactive: plan.interactive,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_continues_to_aur_after_success_or_idle() {
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Success, true),
            RepoPhaseDecision::ContinueToAur
        ));
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Stopped { idle: true }, true),
            RepoPhaseDecision::ContinueToAur
        ));
    }

    #[test]
    fn repo_finishes_without_aur_or_after_terminal_outcome() {
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Success, false),
            RepoPhaseDecision::Finish(ChildOutcome::Success)
        ));
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Stopped { idle: false }, true),
            RepoPhaseDecision::Finish(ChildOutcome::Stopped { idle: false })
        ));
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Failed("boom".to_string()), true),
            RepoPhaseDecision::Finish(ChildOutcome::Failed(_))
        ));
        assert!(matches!(
            repo_phase_decision(&ChildOutcome::Dismissed, false),
            RepoPhaseDecision::Finish(ChildOutcome::Dismissed)
        ));
    }
}
