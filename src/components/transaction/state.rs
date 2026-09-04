use std::collections::{HashMap, HashSet};

use pakajo::events::InstallEvent;
use pakajo::install::ChildOutcome;
use pakajo::package::PackageSource;
use pakajo::transaction_state::{
    InstallKind, RepoStage, RepoState, apply_repo_counters, event_stage, ordered_stages,
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
    pub(crate) expanded: HashSet<usize>,
    pub(crate) status: TransactionStatus,
    pub(crate) source: PackageSource,
    pub(crate) kind: InstallKind,
    pub(super) review: Option<ReviewModel>,
    pub(super) pending_approvals: Option<String>,
    pub(super) pkgbuild_review: Option<PkgbuildModel>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
            repo_state: RepoState {
                manifest: None,
                resolve_started: false,
                resolve_checking: false,
                stage: RepoStage::Resolve,
                download_total: 0,
                download_done: 0,
                download_bytes_total: 0,
                download_bytes_done: 0,
                download_files: HashMap::new(),
                download_order: Vec::new(),
                download_rate: 0.0,
                download_sync_time: None,
                download_sync_done: 0,
            },
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
        if self.stage_state(i) == StageState::Done && !self.expanded.insert(i) {
            self.expanded.remove(&i);
        }
    }
}
