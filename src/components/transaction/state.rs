use std::collections::HashSet;

use pakajo::dispatch::exec::ChildOutcome;
use pakajo::events::InstallEvent;
use pakajo::package::PackageSource;
use pakajo::progress::{
    AurStage, AurState, BuildStatus, DownloadState, InstallKind, InstallState, RepoStage,
    RepoState, VALIDATE_TOTAL, apply_aur_counters, apply_repo_counters, event_stage, finish_aur,
    ordered_aur_stages, ordered_stages,
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
    pub(crate) stages: &'static [RepoStage],
    pub(crate) current_idx: usize,
    pub(crate) repo_state: RepoState,
    pub(crate) aur: AurState,
    pub(crate) expanded: HashSet<usize>,
    pub(crate) expanded_cards: HashSet<String>,
    pub(crate) status: TransactionStatus,
    pub(crate) source: PackageSource,
    pub(crate) kind: InstallKind,
    pub(crate) now: std::time::Instant,
    pub(super) review: Option<ReviewModel>,
    pub(super) pending_approvals: Option<String>,
    pub(super) pkgbuild_review: Option<PkgbuildModel>,
    pub(super) failure_message: Option<String>,
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
            expanded_cards: HashSet::new(),
            status: TransactionStatus::Checking,
            source,
            kind,
            now: std::time::Instant::now(),
            review: None,
            pending_approvals: None,
            pkgbuild_review: None,
            failure_message: None,
        }
    }

    pub(crate) fn apply_event(&mut self, ev: &InstallEvent) {
        if self.is_aur() {
            apply_aur_counters(&mut self.aur, ev, std::time::Instant::now());
            return;
        }
        apply_repo_counters(&mut self.repo_state, ev, std::time::Instant::now());
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
        if self.is_aur() {
            finish_aur(&mut self.aur, &outcome, std::time::Instant::now());
        }
        if let ChildOutcome::Failed(message) = &outcome {
            self.failure_message = Some(message.clone());
        }
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

    pub(crate) fn toggle_build_card(&mut self, name: String) {
        if !self.expanded_cards.insert(name.clone()) {
            self.expanded_cards.remove(&name);
        }
    }

    pub(crate) fn tick(&mut self, now: std::time::Instant) {
        self.now = now;
    }

    pub(crate) fn building(&self) -> bool {
        self.is_aur() && self.aur.building()
    }

    pub(crate) fn overall_progress(&self) -> f32 {
        match &self.status {
            TransactionStatus::Checking => 0.0,
            TransactionStatus::Done(ChildOutcome::Success) => 100.0,
            TransactionStatus::Running | TransactionStatus::Done(_) => {
                if self.is_aur() {
                    self.aur_overall()
                } else {
                    self.repo_overall()
                }
            }
        }
        .clamp(0.0, 100.0)
    }

    fn repo_overall(&self) -> f32 {
        let weight = |stage: RepoStage| match stage {
            RepoStage::Resolve | RepoStage::Validate | RepoStage::Finalize => 5.0,
            RepoStage::Download => 40.0,
            RepoStage::Install => 45.0,
        };
        let total: f32 = self.stages.iter().map(|s| weight(*s)).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let current = self.current_idx.min(self.stages.len());
        let completed: f32 = self.stages.iter().take(current).map(|s| weight(*s)).sum();
        let Some(stage) = self.stages.get(current) else {
            return 100.0;
        };
        let repo = &self.repo_state;
        let frac = match *stage {
            RepoStage::Resolve => repo.resolve_step() as f32 / 3.0,
            RepoStage::Validate => repo.validate.count() as f32 / VALIDATE_TOTAL as f32,
            RepoStage::Download => download_fraction(&repo.download),
            RepoStage::Install => {
                let expected = repo
                    .manifest
                    .as_ref()
                    .map_or(0, |manifest| manifest.packages.len());
                install_fraction(&repo.install, expected)
            }
            RepoStage::Finalize => 0.0,
        }
        .clamp(0.0, 1.0);
        ((completed + weight(*stage) * frac) / total * 100.0).clamp(0.0, 100.0)
    }

    fn aur_builds_done(&self) -> bool {
        !self.aur.build_order.is_empty()
            && self
                .aur
                .build_order
                .iter()
                .filter_map(|name| self.aur.builds.get(name))
                .all(|entry| entry.status == BuildStatus::Done)
    }

    fn aur_overall(&self) -> f32 {
        let aur = &self.aur;
        if !aur.resolve_complete {
            return 0.0;
        }
        if !self.aur_builds_done() {
            return 5.0;
        }
        if !aur.finalize.lines.is_empty() {
            return 95.0;
        }
        let frac = if !aur.install.order.is_empty() {
            install_fraction(&aur.install, aur.build_order.len() + aur.download.total)
        } else if aur.download.total > 0 {
            download_fraction(&aur.download)
        } else {
            0.0
        };
        (60.0 + 35.0 * frac).clamp(0.0, 100.0)
    }

    pub(crate) fn is_aur(&self) -> bool {
        matches!(self.source, PackageSource::Aur) && self.kind != InstallKind::Remove
    }

    pub(crate) fn is_sysupgrade(&self) -> bool {
        self.kind == InstallKind::Upgrade
    }

    fn aur_failed_at(&self, stage: AurStage) -> bool {
        matches!(self.status, TransactionStatus::Done(_))
            && !matches!(self.status, TransactionStatus::Done(ChildOutcome::Success))
            && self.aur.last_aur_stage == Some(stage)
    }

    pub(crate) fn aur_stage_state(&self, stage: AurStage) -> StageState {
        use AurStage::*;
        if matches!(self.status, TransactionStatus::Checking) {
            return StageState::Pending;
        }
        let done_success = matches!(self.status, TransactionStatus::Done(ChildOutcome::Success));
        match stage {
            Resolve => {
                if !self.aur.resolve_started {
                    StageState::Pending
                } else if self.aur.resolve_complete {
                    StageState::Done
                } else if self.aur_failed_at(Resolve) {
                    StageState::Failed
                } else {
                    StageState::Active
                }
            }
            Build => {
                if self.aur.build_order.is_empty() {
                    StageState::Pending
                } else if self.aur_builds_done() {
                    StageState::Done
                } else if self.aur_failed_at(Build) {
                    StageState::Failed
                } else {
                    StageState::Active
                }
            }
            Install => {
                if self.aur_failed_at(Install) {
                    StageState::Failed
                } else if done_success {
                    StageState::Done
                } else if !self.aur.install.order.is_empty()
                    || self.aur.download.total > 0
                    || !self.aur.finalize.lines.is_empty()
                {
                    StageState::Active
                } else {
                    StageState::Pending
                }
            }
            Finalize => {
                if self.aur_failed_at(Finalize) {
                    StageState::Failed
                } else if done_success {
                    StageState::Done
                } else if !self.aur.finalize.lines.is_empty() {
                    StageState::Active
                } else {
                    StageState::Pending
                }
            }
        }
    }
}

fn download_fraction(state: &DownloadState) -> f32 {
    if state.bytes_total > 0 {
        return (state.bytes_done as f32 / state.bytes_total as f32).clamp(0.0, 1.0);
    }
    if state.total > 0 {
        return (state.done as f32 / state.total as f32).clamp(0.0, 1.0);
    }
    0.0
}

fn install_fraction(state: &InstallState, expected: usize) -> f32 {
    let finished = state.packages.values().filter(|pkg| pkg.completed).count();
    let total = expected.max(state.order.len());
    if total == 0 {
        return 0.0;
    }
    (finished as f32 / total as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pakajo::events::LogLevel;
    use pakajo::events::{PackageOp, ProgressPhase};

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

    #[test]
    fn aur_build_failure_attributed_to_build() {
        let mut model = aur_model();
        model.apply_event(&InstallEvent::ResolvingAurDependencies {
            target: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "yay".to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::ResolutionComplete {
            layers: 1,
            aur_packages: 1,
            repo_deps: 0,
        });
        model.apply_event(&InstallEvent::CloningRepo {
            package: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::BuildStarted {
            package: "yay".to_string(),
        });
        model.finish(ChildOutcome::Failed("makepkg failed".to_string()));
        assert_eq!(model.failure_message.as_deref(), Some("makepkg failed"));
        assert_eq!(model.aur_stage_state(AurStage::Resolve), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Failed);
        assert_eq!(
            model.aur_stage_state(AurStage::Install),
            StageState::Pending
        );
        assert_eq!(
            model.aur_stage_state(AurStage::Finalize),
            StageState::Pending
        );
        assert_eq!(
            model.aur.builds.get("yay").expect("yay present").status,
            BuildStatus::Failed
        );
    }

    #[test]
    fn aur_nested_install_failure_keeps_build_done() {
        let mut model = aur_model();
        model.apply_event(&InstallEvent::ResolvingAurDependencies {
            target: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "yay".to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::ResolutionComplete {
            layers: 1,
            aur_packages: 1,
            repo_deps: 0,
        });
        model.apply_event(&InstallEvent::CloningRepo {
            package: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::BuildStarted {
            package: "yay".to_string(),
        });
        model.apply_event(&InstallEvent::BuildCompleted {
            package: "yay".to_string(),
            artifacts: Vec::new(),
            version: None,
        });
        model.finish(ChildOutcome::Failed("pkexec dismissed".to_string()));
        assert_eq!(model.aur_stage_state(AurStage::Resolve), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Done);
        assert_eq!(
            model.aur_stage_state(AurStage::Install),
            StageState::Pending
        );
    }

    #[test]
    fn aur_nested_installs_single_layer() {
        use pakajo::events::DownloadResult;
        let mut model = aur_model();
        model.apply_event(&InstallEvent::ResolvingAurDependencies {
            target: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "dep1".to_string(),
            repo: Some("extra".to_string()),
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "pkg-a".to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::ResolutionComplete {
            layers: 1,
            aur_packages: 1,
            repo_deps: 1,
        });
        model.apply_event(&InstallEvent::RetrievingPackages {
            num: 1,
            total_bytes: 100,
        });
        model.apply_event(&InstallEvent::DownloadInit {
            filename: "dep1".to_string(),
            optional: false,
        });
        model.apply_event(&InstallEvent::DownloadCompleted {
            filename: "dep1".to_string(),
            total: 100,
            result: DownloadResult::Success,
        });
        model.apply_event(&InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: "dep1".to_string(),
            new_version: Some("1.0-1".to_string()),
            old_version: None,
        });
        model.apply_event(&InstallEvent::Progress {
            phase: ProgressPhase::Add,
            package: "dep1".to_string(),
            percent: 100,
            current: 1,
            total: 1,
        });
        model.apply_event(&InstallEvent::CloningRepo {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildStarted {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildOutput {
            package: "pkg-a".to_string(),
            line: "==> Making package".to_string(),
        });
        model.apply_event(&InstallEvent::BuildCompleted {
            package: "pkg-a".to_string(),
            artifacts: Vec::new(),
            version: None,
        });
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Install), StageState::Active);
        model.apply_event(&InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: "pkg-a".to_string(),
            new_version: Some("1.0-1".to_string()),
            old_version: None,
        });
        assert_eq!(model.aur.install.order.len(), 2);
        model.apply_event(&InstallEvent::HookRun {
            position: 1,
            total: 1,
            name: "hook".to_string(),
            desc: Some("Arming ConditionNeedsUpdate...".to_string()),
        });
        model.apply_event(&InstallEvent::TransactionDone);
        model.finish(ChildOutcome::Success);
        assert_eq!(model.aur_stage_state(AurStage::Resolve), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Install), StageState::Done);
        assert_eq!(model.aur_stage_state(AurStage::Finalize), StageState::Done);
    }

    #[test]
    fn aur_install_failure_preserves_checklist() {
        use pakajo::events::DownloadResult;
        let mut model = aur_model();
        model.apply_event(&InstallEvent::ResolvingAurDependencies {
            target: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "dep1".to_string(),
            repo: Some("extra".to_string()),
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::AurDepResolved {
            package: "pkg-a".to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        });
        model.apply_event(&InstallEvent::ResolutionComplete {
            layers: 1,
            aur_packages: 1,
            repo_deps: 1,
        });
        model.apply_event(&InstallEvent::RetrievingPackages {
            num: 1,
            total_bytes: 100,
        });
        model.apply_event(&InstallEvent::DownloadInit {
            filename: "dep1".to_string(),
            optional: false,
        });
        model.apply_event(&InstallEvent::DownloadCompleted {
            filename: "dep1".to_string(),
            total: 100,
            result: DownloadResult::Success,
        });
        model.apply_event(&InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: "dep1".to_string(),
            new_version: Some("1.0-1".to_string()),
            old_version: None,
        });
        model.apply_event(&InstallEvent::Progress {
            phase: ProgressPhase::Add,
            package: "dep1".to_string(),
            percent: 100,
            current: 1,
            total: 1,
        });
        model.apply_event(&InstallEvent::CloningRepo {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildStarted {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildCompleted {
            package: "pkg-a".to_string(),
            artifacts: Vec::new(),
            version: None,
        });
        model.apply_event(&InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: "pkg-a".to_string(),
            new_version: Some("1.0-1".to_string()),
            old_version: None,
        });
        model.finish(ChildOutcome::Failed("install failed".to_string()));
        assert_eq!(model.aur_stage_state(AurStage::Install), StageState::Failed);
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Done);
        assert_eq!(model.aur.install.order.len(), 2);
    }

    #[test]
    fn aur_finalize_active_while_hooks_run() {
        let mut model = aur_model();
        model.apply_event(&InstallEvent::CloningRepo {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildCompleted {
            package: "pkg-a".to_string(),
            artifacts: Vec::new(),
            version: None,
        });
        assert_eq!(model.aur_stage_state(AurStage::Build), StageState::Done);
        assert_eq!(
            model.aur_stage_state(AurStage::Finalize),
            StageState::Pending
        );
        model.apply_event(&InstallEvent::HookRun {
            position: 1,
            total: 2,
            name: "update-desktop-database".to_string(),
            desc: None,
        });
        assert_eq!(
            model.aur_stage_state(AurStage::Finalize),
            StageState::Active
        );
        model.finish(ChildOutcome::Success);
        assert_eq!(model.aur_stage_state(AurStage::Finalize), StageState::Done);
    }

    #[test]
    fn log_alerts_do_not_advance_stage_state() {
        let mut model = aur_model();
        model.apply_event(&InstallEvent::CloningRepo {
            package: "pkg-a".to_string(),
        });
        model.apply_event(&InstallEvent::BuildCompleted {
            package: "pkg-a".to_string(),
            artifacts: Vec::new(),
            version: None,
        });
        assert_eq!(
            model.aur_stage_state(AurStage::Finalize),
            StageState::Pending
        );
        model.apply_event(&InstallEvent::Log {
            level: LogLevel::Error,
            message: "key unknown\n".to_string(),
        });
        assert_eq!(
            model.aur_stage_state(AurStage::Finalize),
            StageState::Pending
        );
        assert_eq!(model.aur.finalize.alerts.len(), 1);
        assert!(!model.aur.finalize.is_empty());
    }
}
