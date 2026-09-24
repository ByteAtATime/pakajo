use futures::SinkExt as _;
use futures::StreamExt as _;

use crate::dispatch::exec::{ChildOutcome, StreamItem, send_done};
use crate::dispatch::operation::{BuildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;

pub struct PhasePlan {
    pub privileged: Option<PrivilegedOperation>,
    pub repo_committed: bool,
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

pub(crate) enum ForwardEnd {
    Done(ChildOutcome),
    InnerEnded,
    ReceiverGone,
}

#[derive(Clone, Debug)]
pub enum RepoPhase {
    Committed,
    Declined,
    Idle,
    Failed(ChildOutcome),
}

#[derive(Clone, Debug)]
pub enum AfterRepo {
    BuildAur,
    Done(ChildOutcome),
    NothingToDo,
}

pub fn after_repo(repo: &RepoPhase, aur_targets: &[String]) -> AfterRepo {
    if let RepoPhase::Failed(outcome) = repo {
        return AfterRepo::Done(outcome.clone());
    }
    if !aur_targets.is_empty() {
        return AfterRepo::BuildAur;
    }
    match repo {
        RepoPhase::Idle => AfterRepo::NothingToDo,
        _ => AfterRepo::Done(ChildOutcome::Success),
    }
}

pub fn repo_phase_from_result(result: anyhow::Result<crate::tx::driver::RunOutcome>) -> RepoPhase {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => return RepoPhase::Failed(ChildOutcome::Failed(format!("{error:#}"))),
    };
    match outcome.finish {
        crate::tx::driver::Finish::Committed => RepoPhase::Committed,
        crate::tx::driver::Finish::Stopped if outcome.summary.is_empty() => RepoPhase::Idle,
        crate::tx::driver::Finish::Stopped => RepoPhase::Declined,
        crate::tx::driver::Finish::PrepareFailed(failure) => RepoPhase::Failed(
            ChildOutcome::Failed(format!("failed to prepare transaction: {failure}")),
        ),
    }
}

pub fn repo_phase_from_child(outcome: &ChildOutcome) -> RepoPhase {
    match outcome {
        ChildOutcome::Success => RepoPhase::Committed,
        ChildOutcome::Stopped { idle: true } => RepoPhase::Idle,
        ChildOutcome::Stopped { idle: false } => RepoPhase::Declined,
        _ => RepoPhase::Failed(outcome.clone()),
    }
}

pub fn aur_failure_warning(
    repo_committed: bool,
    privileged_present: bool,
    repo_verb: &str,
    reason: &str,
) -> Option<String> {
    if !repo_committed && !privileged_present {
        return None;
    }
    Some(format!(
        "repo packages {repo_verb}; AUR phase failed: {reason}"
    ))
}

pub fn run_phases(plan: PhasePlan, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    if plan.privileged.is_none() && plan.aur_targets.is_empty() {
        send_done(tx, ChildOutcome::Failed("no install targets".to_string()));
        return;
    }
    let privileged_present = plan.privileged.is_some();
    let mut repo_committed = plan.repo_committed;
    if let Some(operation) = plan.privileged {
        repo_committed = true;
        let outcome = match forward(operation.dispatch(plan.tty), tx) {
            ForwardEnd::Done(outcome) => outcome,
            ForwardEnd::InnerEnded => {
                send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
                return;
            }
            ForwardEnd::ReceiverGone => return,
        };
        match after_repo(&repo_phase_from_child(&outcome), &plan.aur_targets) {
            AfterRepo::BuildAur => {}
            AfterRepo::Done(outcome) => {
                send_done(tx, outcome);
                return;
            }
            AfterRepo::NothingToDo => {
                send_done(tx, ChildOutcome::Success);
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
            if let Some(warning) = aur_failure_warning(
                repo_committed,
                privileged_present,
                plan.repo_verb,
                outcome.reason(),
            ) {
                eprintln!("warning: {warning}");
            }
            send_done(tx, outcome);
        }
        ForwardEnd::InnerEnded => {
            if let Some(warning) = aur_failure_warning(
                repo_committed,
                privileged_present,
                plan.repo_verb,
                "stream ended",
            ) {
                eprintln!("warning: {warning}");
            }
            send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
        }
        ForwardEnd::ReceiverGone => {}
    }
}

pub(crate) fn forward(
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

    fn declined() -> RepoPhase {
        RepoPhase::Declined
    }

    fn idle() -> RepoPhase {
        RepoPhase::Idle
    }

    #[test]
    fn failed_with_pending_aur_finishes_preserving_payload() {
        let outcome = ChildOutcome::Failed("boom".to_string());
        let targets = vec!["pipes.sh".to_string()];
        let decision = after_repo(&RepoPhase::Failed(outcome.clone()), &targets);
        assert!(matches!(decision, AfterRepo::Done(done) if done == outcome));
    }

    #[test]
    fn idle_without_aur_has_nothing_to_do() {
        assert!(matches!(after_repo(&idle(), &[]), AfterRepo::NothingToDo));
    }

    #[test]
    fn declined_with_pending_aur_still_builds() {
        let targets = vec!["pipes.sh".to_string()];
        assert!(matches!(
            after_repo(&declined(), &targets),
            AfterRepo::BuildAur
        ));
    }

    #[test]
    fn result_stopped_classifies_idle_vs_declined() {
        use crate::tx::driver::{Finish, RunOutcome};
        let idle = RunOutcome {
            summary: crate::events::TransactionSummary::default(),
            finish: Finish::Stopped,
            review: None,
        };
        assert!(matches!(repo_phase_from_result(Ok(idle)), RepoPhase::Idle));
        let declined = RunOutcome {
            summary: crate::events::TransactionSummary {
                packages: vec![crate::events::SummaryPackage {
                    name: "foo".to_string(),
                    repository: None,
                    new_version: "1.0".to_string(),
                    old_version: None,
                    download_size: 0,
                    installed_size: 0,
                    old_installed_size: 0,
                    is_removal: false,
                }],
                total_download_size: 0,
                total_installed_size: 0,
                total_removed_size: 0,
            },
            finish: Finish::Stopped,
            review: None,
        };
        assert!(matches!(
            repo_phase_from_result(Ok(declined)),
            RepoPhase::Declined
        ));
    }

    #[test]
    fn child_failures_map_to_failed_preserving_payload() {
        for outcome in [
            ChildOutcome::Failed("boom".to_string()),
            ChildOutcome::Dismissed,
            ChildOutcome::NotFound("sudo not found".to_string()),
        ] {
            let phase = repo_phase_from_child(&outcome);
            assert!(
                matches!(phase, RepoPhase::Failed(preserved) if preserved == outcome),
                "must preserve {outcome:?}"
            );
        }
    }

    #[test]
    fn aur_failure_warning_reports_progressed_repo_only() {
        let warning = aur_failure_warning(true, false, "upgraded", "build broke");
        assert_eq!(
            warning.as_deref(),
            Some("repo packages upgraded; AUR phase failed: build broke")
        );
        assert_eq!(
            aur_failure_warning(false, false, "upgraded", "build broke"),
            None
        );
    }
}
