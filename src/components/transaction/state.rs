use std::collections::HashSet;

use pakajo::events::InstallEvent;
use pakajo::install::ChildOutcome;
use pakajo::package::PackageSource;
use pakajo::transaction_state::{
    AurStage, AurState, InstallKind, RepoStage, RepoState, apply_aur_counters, apply_repo_counters,
    event_stage, ordered_aur_stages, ordered_stages,
};

use super::pkgbuild::PkgbuildModel;
use super::review::ReviewModel;

#[derive(Clone, Debug)]
pub(crate) enum TransactionStatus {
    Checking,
    Running,
    Done(ChildOutcome),
}

pub(crate) struct TransactionModel {
    pub(crate) name: String,
    pub(crate) stages: Vec<RepoStage>,
    pub(crate) current_idx: usize,
    pub(crate) repo_state: RepoState,
    pub(crate) aur: AurState,
    pub(crate) expanded: HashSet<usize>,
    pub(crate) status: TransactionStatus,
    pub(crate) source: PackageSource,
    pub(crate) kind: InstallKind,
    pub(super) review: Option<ReviewModel>,
    pub(super) pending_approvals: Option<String>,
    pub(super) pkgbuild_review: Option<PkgbuildModel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageState {
    Pending,
    Active,
    Done,
    Failed,
}

impl TransactionModel {
    pub(crate) fn new(name: String, source: PackageSource, kind: InstallKind) -> Self {
        Self {
            name,
            stages: ordered_stages(kind),
            current_idx: 0,
            repo_state: RepoState::default(),
            aur: AurState::default(),
            expanded: HashSet::new(),
            status: TransactionStatus::Checking,
            source,
            kind,
            review: None,
            pending_approvals: None,
            pkgbuild_review: None,
        }
    }

    pub(crate) fn apply_event(&mut self, ev: &InstallEvent) {
        if self.is_aur() {
            apply_aur_counters(&mut self.aur, ev);
            return;
        }
        apply_repo_counters(&mut self.repo_state, ev);
        if matches!(ev, InstallEvent::TransactionSummary(_)) {
            self.leave_resolve();
            return;
        }
        if let Some(stage) = event_stage(ev)
            && let Some(idx) = self.stages.iter().position(|s| *s == stage)
            && idx > self.current_idx
            && !self.awaiting_manifest()
        {
            self.current_idx = idx;
        }
    }

    fn leave_resolve(&mut self) {
        if self.current_idx == 0 && self.stages.first() == Some(&RepoStage::Resolve) {
            self.current_idx = 1;
        }
    }

    fn awaiting_manifest(&self) -> bool {
        self.current_idx == 0
            && self.stages.first() == Some(&RepoStage::Resolve)
            && self.repo_state.manifest.is_none()
    }

    pub(crate) fn finish(&mut self, outcome: ChildOutcome) {
        if matches!(outcome, ChildOutcome::Success) {
            self.current_idx = self.stages.len();
        }
        self.status = TransactionStatus::Done(outcome);
    }

    pub(crate) fn stage_state(&self, i: usize) -> StageState {
        if i < self.current_idx {
            StageState::Done
        } else if i == self.current_idx {
            match &self.status {
                TransactionStatus::Checking => StageState::Pending,
                TransactionStatus::Running => StageState::Active,
                TransactionStatus::Done(ChildOutcome::Success) => StageState::Done,
                TransactionStatus::Done(_) => StageState::Failed,
            }
        } else {
            StageState::Pending
        }
    }

    pub(crate) fn toggle(&mut self, i: usize) {
        let done = if self.is_aur() {
            ordered_aur_stages()
                .get(i)
                .is_some_and(|stage| self.aur_stage_state(*stage) == StageState::Done)
        } else {
            self.stage_state(i) == StageState::Done
        };
        if done && !self.expanded.insert(i) {
            self.expanded.remove(&i);
        }
    }

    pub(crate) fn is_aur(&self) -> bool {
        matches!(self.source, PackageSource::Aur) && self.kind != InstallKind::Remove
    }

    pub(crate) fn aur_stage_state(&self, stage: AurStage) -> StageState {
        use AurStage::*;
        if matches!(self.status, TransactionStatus::Checking) {
            return StageState::Pending;
        }
        let done_success = matches!(self.status, TransactionStatus::Done(ChildOutcome::Success));
        let done_failed = matches!(self.status, TransactionStatus::Done(_)) && !done_success;
        match stage {
            Resolve => {
                if !self.aur.resolve_started {
                    StageState::Pending
                } else if self.aur.resolve_complete {
                    StageState::Done
                } else if done_failed {
                    StageState::Failed
                } else {
                    StageState::Active
                }
            }
            Build | Install | Finalize => StageState::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aur_model() -> TransactionModel {
        let mut model =
            TransactionModel::new("yay".to_string(), PackageSource::Aur, InstallKind::Install);
        model.status = TransactionStatus::Running;
        model
    }

    #[test]
    fn aur_resolve_failure() {
        let mut model = aur_model();
        model.apply_event(&InstallEvent::ResolvingAurDependencies {
            target: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "yay".to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        });
        model.finish(ChildOutcome::Failed("resolve failed".to_string()));
        assert_eq!(model.aur_stage_state(AurStage::Resolve), StageState::Failed);
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Pending);
    }
}
