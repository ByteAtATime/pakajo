use std::collections::HashSet;
use std::time::Instant;

use pakajo::dispatch::exec::{AnswerWriter, ChildOutcome};
use pakajo::download::TransferState;
use pakajo::events::{InstallEvent, TransactionSummary};
use pakajo::package::PackageSource;
use pakajo::progress::{
    AurPhase, AurStage, AurState, BuildStatus, InstallKind, InstallState, RepoStage, RepoState,
    VALIDATE_TOTAL, apply_aur_counters, apply_repo_counters, event_stage, finish_aur,
    ordered_aur_stages, ordered_stages,
};
use pakajo::question::model::Question;
use pakajo::upgrade::AurUpgradeCandidate;

use super::pkgbuild::PkgbuildModel;
use super::removal::RemovalConfirmModel;
use super::review::InstallReview;

#[derive(Clone, Debug)]
pub(crate) enum TransactionStatus {
    Checking,
    Running,
    Done(ChildOutcome),
}

pub(crate) struct TransactionModel {
    pub(crate) name: String,
    pub(crate) targets: Vec<String>,
    pub(crate) aur_names: Vec<String>,
    pub(crate) stages: &'static [RepoStage],
    pub(crate) current_idx: usize,
    pub(crate) repo_state: RepoState,
    pub(crate) aur: AurState,
    pub(crate) expanded: HashSet<usize>,
    pub(crate) expanded_cards: HashSet<String>,
    pub(crate) status: TransactionStatus,
    pub(crate) source: PackageSource,
    pub(crate) kind: InstallKind,
    pub(crate) now: Instant,
    pub(super) install_review: Option<InstallReview>,
    pub(super) summary: Option<TransactionSummary>,
    pub(super) removal_confirm: Option<RemovalConfirmModel>,
    pub(super) remove_description: Option<String>,
    pub(super) remove_repo: Option<String>,
    pub(super) pending_approvals: Option<String>,
    pub(super) pkgbuild_review: Option<PkgbuildModel>,
    pub(super) failure_message: Option<String>,
    pub(super) answer_channel: Option<AnswerWriter>,
    pub(super) pending_import_key: Option<Question>,
    pub(super) revalidations: usize,
    pub(super) unstables: usize,
    pub(super) review_notice: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageState {
    Pending,
    Active,
    Done,
    Failed,
}

impl TransactionModel {
    pub(crate) fn batch(
        name: String,
        targets: Vec<String>,
        aur_names: Vec<String>,
        kind: InstallKind,
    ) -> Self {
        let source = if aur_names.is_empty() {
            PackageSource::Repo
        } else {
            PackageSource::Aur
        };
        Self {
            name,
            targets,
            aur_names,
            stages: ordered_stages(kind),
            current_idx: 0,
            repo_state: RepoState::default(),
            aur: AurState::default(),
            expanded: HashSet::new(),
            expanded_cards: HashSet::new(),
            status: TransactionStatus::Checking,
            source,
            kind,
            now: Instant::now(),
            install_review: None,
            summary: None,
            removal_confirm: None,
            remove_description: None,
            remove_repo: None,
            pending_approvals: None,
            pkgbuild_review: None,
            failure_message: None,
            answer_channel: None,
            pending_import_key: None,
            revalidations: 0,
            unstables: 0,
            review_notice: None,
        }
    }

    pub(crate) fn new(name: String, source: PackageSource, kind: InstallKind) -> Self {
        let targets = vec![name.clone()];
        let is_removal = kind == InstallKind::Remove;
        let aur_names = match source {
            PackageSource::Aur if !is_removal => vec![name.clone()],
            _ => Vec::new(),
        };
        Self::batch(name, targets, aur_names, kind)
    }

    pub(crate) fn apply_event(&mut self, ev: &InstallEvent) {
        if let InstallEvent::FailClosed { key, reason } = ev {
            self.failure_message = Some(format!("{key:?}: {reason}"));
        }
        if let InstallEvent::RuntimePrompt { question } = ev
            && self.answer_channel.is_some()
        {
            self.pending_import_key = Some(question.clone());
        }
        let now = Instant::now();
        if self.is_aur() {
            apply_aur_counters(&mut self.aur, ev, now);
            return;
        }
        if self.is_sysupgrade() {
            apply_aur_counters(&mut self.aur, ev, now);
        }
        apply_repo_counters(&mut self.repo_state, ev, now);
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
        if self.tracks_aur() {
            finish_aur(&mut self.aur, &outcome, Instant::now());
        }
        if let ChildOutcome::Failed(message) | ChildOutcome::NotFound(message) = &outcome
            && self.failure_message.is_none()
        {
            self.failure_message = Some(message.clone());
        }
        if matches!(outcome, ChildOutcome::Success) {
            self.current_idx = self.stages.len();
        }
        self.status = TransactionStatus::Done(outcome);
        self.pending_import_key = None;
        self.answer_channel = None;
    }

    pub(crate) fn adopt_aur_candidates(&mut self, candidates: Vec<AurUpgradeCandidate>) {
        self.aur_names = candidates.into_iter().map(|c| c.name).collect();
    }

    pub(crate) fn set_answer_channel(&mut self, writer: AnswerWriter) {
        self.answer_channel = Some(writer);
        self.pending_import_key = None;
    }

    pub(crate) fn answer_import_key(&mut self, yes: bool) {
        if let Some(channel) = self.answer_channel.as_ref() {
            channel.answer(yes);
        }
        self.pending_import_key = None;
    }

    pub(crate) fn build_owns_failure(&self) -> bool {
        self.is_sysupgrade()
            && !self.aur.build_order.is_empty()
            && self.aur_stage_state(AurStage::Build) == StageState::Failed
    }

    pub(crate) fn stage_state(&self, i: usize) -> StageState {
        if i < self.current_idx {
            StageState::Done
        } else if i == self.current_idx {
            match &self.status {
                TransactionStatus::Checking => StageState::Pending,
                TransactionStatus::Running => StageState::Active,
                TransactionStatus::Done(ChildOutcome::Success) => StageState::Done,
                TransactionStatus::Done(_) if self.build_owns_failure() => StageState::Done,
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
        } else if self.is_sysupgrade() && i == self.stages.len() {
            self.aur_stage_state(AurStage::Build) == StageState::Done
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

    fn tracks_aur(&self) -> bool {
        self.is_aur() || self.is_sysupgrade()
    }

    fn done_without_success(&self) -> bool {
        matches!(self.status, TransactionStatus::Done(_))
            && !matches!(self.status, TransactionStatus::Done(ChildOutcome::Success))
    }

    pub(crate) fn building(&self) -> bool {
        self.tracks_aur() && self.aur.building()
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
        let total: f32 = self.stages.iter().map(|s| repo_stage_weight(*s)).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let current = self.current_idx.min(self.stages.len());
        let completed: f32 = self
            .stages
            .iter()
            .take(current)
            .map(|s| repo_stage_weight(*s))
            .sum();
        let Some(stage) = self.stages.get(current) else {
            return 100.0;
        };
        let frac = self.repo_stage_fraction(*stage);
        ((completed + repo_stage_weight(*stage) * frac) / total * 100.0).clamp(0.0, 100.0)
    }

    fn repo_stage_fraction(&self, stage: RepoStage) -> f32 {
        let repo = &self.repo_state;
        match stage {
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
        .clamp(0.0, 1.0)
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
        if let Some(frozen) = self.aur_failure_waypoint() {
            return frozen;
        }
        self.aur_running_waypoint()
    }

    fn aur_failure_waypoint(&self) -> Option<f32> {
        if !self.done_without_success() {
            return None;
        }
        match self.aur.last_aur_stage {
            Some(AurStage::Resolve) => Some(5.0),
            Some(AurStage::Deps) => Some(5.0 + 25.0 * self.aur_deps_fraction()),
            Some(AurStage::Build) => Some(30.0),
            Some(AurStage::Install) | Some(AurStage::Finalize) => {
                Some(65.0 + 35.0 * self.aur_install_fraction())
            }
            None => None,
        }
    }

    fn aur_running_waypoint(&self) -> f32 {
        if !self.aur.resolve_complete {
            return 0.0;
        }
        if self.aur.phase == AurPhase::Deps {
            return 5.0 + 25.0 * self.aur_deps_fraction();
        }
        if !self.aur_builds_done() {
            return 30.0;
        }
        65.0 + 35.0 * self.aur_install_fraction()
    }

    fn aur_deps_fraction(&self) -> f32 {
        if !self.aur.repo_deps.install.order.is_empty() {
            return install_fraction(&self.aur.repo_deps.install, self.aur.repo_dep_count);
        }
        download_fraction(&self.aur.repo_deps.download)
    }

    fn aur_install_fraction(&self) -> f32 {
        if !self.aur.install.order.is_empty() {
            return install_fraction(
                &self.aur.install,
                self.aur.build_order.len() + self.aur.download.total,
            );
        }
        if self.aur.download.total > 0 {
            return download_fraction(&self.aur.download);
        }
        0.0
    }

    pub(crate) fn is_aur(&self) -> bool {
        matches!(self.source, PackageSource::Aur) && self.kind != InstallKind::Remove
    }

    pub(crate) fn is_sysupgrade(&self) -> bool {
        self.kind == InstallKind::Upgrade
    }

    fn aur_failed_at(&self, stage: AurStage) -> bool {
        self.done_without_success() && self.aur.last_aur_stage == Some(stage)
    }

    pub(crate) fn aur_stage_state(&self, stage: AurStage) -> StageState {
        use AurStage::*;
        if matches!(self.status, TransactionStatus::Checking) {
            return StageState::Pending;
        }
        let succeeded = matches!(self.status, TransactionStatus::Done(ChildOutcome::Success));
        let active_or_pending = |active: bool| {
            if active {
                StageState::Active
            } else {
                StageState::Pending
            }
        };
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
            Deps => {
                if self.aur_failed_at(Deps) {
                    StageState::Failed
                } else if succeeded || self.aur.phase == AurPhase::Artifacts {
                    StageState::Done
                } else {
                    active_or_pending(!self.deps_bucket_empty())
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
                    active_or_pending(self.aur.phase == AurPhase::Artifacts)
                }
            }
            Install => {
                let aur = &self.aur;
                if self.aur_failed_at(Install) {
                    StageState::Failed
                } else if succeeded {
                    StageState::Done
                } else {
                    active_or_pending(
                        !aur.install.order.is_empty()
                            || aur.download.total > 0
                            || !aur.finalize.lines.is_empty(),
                    )
                }
            }
            Finalize => {
                if self.aur_failed_at(Finalize) {
                    StageState::Failed
                } else if succeeded {
                    StageState::Done
                } else {
                    active_or_pending(!self.aur.finalize.lines.is_empty())
                }
            }
        }
    }

    pub(crate) fn deps_bucket_empty(&self) -> bool {
        self.aur.repo_deps.download.total == 0
            && self.aur.repo_deps.install.order.is_empty()
            && self.aur.repo_deps.finalize.lines.is_empty()
    }

    pub(crate) fn deps_section_visible(&self) -> bool {
        self.aur.repo_dep_count > 0 || !self.deps_bucket_empty()
    }

    pub(crate) fn artifact_sections_visible(&self) -> bool {
        !self.aur.build_order.is_empty()
    }
}

fn repo_stage_weight(stage: RepoStage) -> f32 {
    match stage {
        RepoStage::Resolve | RepoStage::Validate | RepoStage::Finalize => 5.0,
        RepoStage::Download => 40.0,
        RepoStage::Install => 45.0,
    }
}

fn download_fraction(state: &TransferState) -> f32 {
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
    use pakajo::events::{DownloadResult, LogLevel, PackageOp, ProgressPhase};
    use pakajo::question::model::QuestionKey;
    use std::process::{Command, Stdio};

    fn aur_model() -> TransactionModel {
        let mut model =
            TransactionModel::new("yay".to_string(), PackageSource::Aur, InstallKind::Install);
        model.status = TransactionStatus::Running;
        model
    }

    fn resolving(target: &str) -> InstallEvent {
        InstallEvent::ResolvingAurDependencies {
            target: target.to_string(),
        }
    }

    fn dep_resolved(package: &str, repo: Option<&str>) -> InstallEvent {
        InstallEvent::AurDepResolved {
            package: package.to_string(),
            repo: repo.map(str::to_string),
            version: Some("1.0-1".to_string()),
        }
    }

    fn resolution_complete(repo_deps: usize) -> InstallEvent {
        InstallEvent::ResolutionComplete {
            aur_packages: 1,
            repo_deps,
        }
    }

    fn retrieving(num: usize) -> InstallEvent {
        InstallEvent::RetrievingPackages {
            num,
            total_bytes: 100,
        }
    }

    fn download_init(filename: &str) -> InstallEvent {
        InstallEvent::DownloadInit {
            filename: filename.to_string(),
            optional: false,
        }
    }

    fn download_progress(filename: &str, downloaded: i64, total: i64) -> InstallEvent {
        InstallEvent::DownloadProgress {
            filename: filename.to_string(),
            downloaded,
            total,
        }
    }

    fn download_completed(filename: &str) -> InstallEvent {
        InstallEvent::DownloadCompleted {
            filename: filename.to_string(),
            total: 100,
            result: DownloadResult::Success,
        }
    }

    fn installed(package: &str) -> InstallEvent {
        InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: package.to_string(),
            new_version: Some("1.0-1".to_string()),
            old_version: None,
        }
    }

    fn progressed(package: &str, percent: i32) -> InstallEvent {
        InstallEvent::Progress {
            phase: ProgressPhase::Add,
            package: package.to_string(),
            percent,
            current: 1,
            total: 1,
        }
    }

    fn cloning(package: &str) -> InstallEvent {
        InstallEvent::CloningRepo {
            package: package.to_string(),
        }
    }

    fn build_started(package: &str) -> InstallEvent {
        InstallEvent::BuildStarted {
            package: package.to_string(),
        }
    }

    fn build_completed(package: &str) -> InstallEvent {
        InstallEvent::BuildCompleted {
            package: package.to_string(),
            artifacts: Vec::new(),
            version: None,
        }
    }

    fn hook(position: usize, total: usize) -> InstallEvent {
        InstallEvent::HookRun {
            position,
            total,
            name: "hook".to_string(),
            desc: None,
        }
    }

    fn import_key_event() -> InstallEvent {
        InstallEvent::RuntimePrompt {
            question: Question::ImportKey {
                fingerprint: "ABCDEF".to_string(),
                uid: "Packager <pack@example.com>".to_string(),
            },
        }
    }

    fn drive_resolve_open(model: &mut TransactionModel) {
        model.apply_event(&resolving("pkg-a"));
        model.apply_event(&dep_resolved("dep1", Some("extra")));
        model.apply_event(&dep_resolved("pkg-a", None));
        model.apply_event(&resolution_complete(1));
        model.apply_event(&retrieving(1));
        model.apply_event(&download_init("dep1"));
    }

    fn drive_dep_downloaded(model: &mut TransactionModel) {
        model.apply_event(&download_progress("dep1", 100, 100));
        model.apply_event(&download_completed("dep1"));
    }

    fn drive_dep_registered(model: &mut TransactionModel) {
        model.apply_event(&installed("dep1"));
    }

    fn drive_resolve_with_dep(model: &mut TransactionModel) {
        drive_resolve_open(model);
        drive_dep_downloaded(model);
        drive_dep_registered(model);
    }

    fn drive_dep_progress(model: &mut TransactionModel, percent: i32) {
        model.apply_event(&progressed("dep1", percent));
    }

    fn drive_build_done(model: &mut TransactionModel) {
        model.apply_event(&cloning("pkg-a"));
        model.apply_event(&build_started("pkg-a"));
        model.apply_event(&build_completed("pkg-a"));
    }

    fn drive_artifact_installed(model: &mut TransactionModel) {
        model.apply_event(&installed("pkg-a"));
        model.apply_event(&progressed("pkg-a", 100));
    }

    fn assert_state(model: &TransactionModel, stage: AurStage, expected: StageState) {
        assert_eq!(model.aur_stage_state(stage), expected);
    }

    #[test]
    fn aur_resolve_failure() {
        let mut model = aur_model();
        model.apply_event(&resolving("yay"));
        model.apply_event(&dep_resolved("yay", None));
        model.finish(ChildOutcome::Failed("resolve failed".to_string()));
        assert_state(&model, AurStage::Resolve, StageState::Failed);
        assert_state(&model, AurStage::Build, StageState::Pending);
    }

    #[test]
    fn aur_build_failure_attributed_to_build() {
        let mut model = aur_model();
        model.apply_event(&resolving("yay"));
        model.apply_event(&dep_resolved("yay", None));
        model.apply_event(&resolution_complete(0));
        model.apply_event(&cloning("yay"));
        model.apply_event(&build_started("yay"));
        model.finish(ChildOutcome::Failed("makepkg failed".to_string()));
        assert_eq!(model.failure_message.as_deref(), Some("makepkg failed"));
        assert_state(&model, AurStage::Resolve, StageState::Done);
        assert_state(&model, AurStage::Build, StageState::Failed);
        assert_state(&model, AurStage::Install, StageState::Pending);
        assert_state(&model, AurStage::Finalize, StageState::Pending);
        assert_eq!(
            model.aur.builds.get("yay").expect("yay present").status,
            BuildStatus::Failed
        );
    }

    #[test]
    fn sysupgrade_build_failure_attributed_to_build() {
        let mut model = TransactionModel::new(
            "system".to_string(),
            PackageSource::Repo,
            InstallKind::Upgrade,
        );
        model.status = TransactionStatus::Running;
        model.apply_event(&InstallEvent::TransactionSummary(TransactionSummary {
            packages: Vec::new(),
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }));
        model.apply_event(&installed("foo"));
        model.apply_event(&hook(1, 1));
        model.apply_event(&cloning("bar"));
        model.apply_event(&build_started("bar"));
        model.finish(ChildOutcome::Failed("makepkg failed".to_string()));
        assert_eq!(model.failure_message.as_deref(), Some("makepkg failed"));
        assert_eq!(model.stage_state(4), StageState::Done);
        assert_state(&model, AurStage::Build, StageState::Failed);
        assert!(model.build_owns_failure());
        assert_eq!(
            model.aur.builds.get("bar").expect("bar present").status,
            BuildStatus::Failed
        );
    }

    #[test]
    fn aur_nested_install_failure_keeps_build_done() {
        let mut model = aur_model();
        model.apply_event(&resolving("yay"));
        model.apply_event(&dep_resolved("yay", None));
        model.apply_event(&resolution_complete(0));
        model.apply_event(&cloning("yay"));
        model.apply_event(&build_started("yay"));
        model.apply_event(&build_completed("yay"));
        model.finish(ChildOutcome::Failed("pkexec dismissed".to_string()));
        assert_state(&model, AurStage::Resolve, StageState::Done);
        assert_state(&model, AurStage::Build, StageState::Done);
        assert_state(&model, AurStage::Install, StageState::Pending);
    }

    #[test]
    fn aur_nested_installs_single_layer() {
        let mut model = aur_model();
        drive_resolve_open(&mut model);
        drive_dep_downloaded(&mut model);
        drive_dep_registered(&mut model);
        drive_dep_progress(&mut model, 100);
        model.apply_event(&cloning("pkg-a"));
        model.apply_event(&build_started("pkg-a"));
        model.apply_event(&InstallEvent::BuildOutput {
            package: "pkg-a".to_string(),
            line: "==> Making package".to_string(),
        });
        model.apply_event(&build_completed("pkg-a"));
        assert_state(&model, AurStage::Build, StageState::Done);
        assert_state(&model, AurStage::Install, StageState::Pending);
        model.apply_event(&installed("pkg-a"));
        assert_eq!(model.aur.install.order.len(), 1);
        assert_eq!(model.aur.repo_deps.install.order, ["dep1"]);
        assert_state(&model, AurStage::Install, StageState::Active);
        model.apply_event(&hook(1, 1));
        model.apply_event(&InstallEvent::TransactionDone);
        model.finish(ChildOutcome::Success);
        assert_state(&model, AurStage::Resolve, StageState::Done);
        assert_state(&model, AurStage::Build, StageState::Done);
        assert_state(&model, AurStage::Install, StageState::Done);
        assert_state(&model, AurStage::Finalize, StageState::Done);
    }

    #[test]
    fn aur_install_failure_preserves_checklist() {
        let mut model = aur_model();
        drive_resolve_open(&mut model);
        drive_dep_downloaded(&mut model);
        drive_dep_registered(&mut model);
        drive_dep_progress(&mut model, 100);
        drive_build_done(&mut model);
        model.apply_event(&installed("pkg-a"));
        model.finish(ChildOutcome::Failed("install failed".to_string()));
        assert_state(&model, AurStage::Install, StageState::Failed);
        assert_state(&model, AurStage::Build, StageState::Done);
        assert_eq!(model.aur.install.order.len(), 1);
        assert_eq!(model.aur.repo_deps.install.order, ["dep1"]);
    }

    #[test]
    fn aur_finalize_active_while_hooks_run() {
        let mut model = aur_model();
        model.apply_event(&cloning("pkg-a"));
        model.apply_event(&build_completed("pkg-a"));
        assert_state(&model, AurStage::Build, StageState::Done);
        assert_state(&model, AurStage::Finalize, StageState::Pending);
        model.apply_event(&hook(1, 2));
        assert_state(&model, AurStage::Finalize, StageState::Active);
        model.finish(ChildOutcome::Success);
        assert_state(&model, AurStage::Finalize, StageState::Done);
    }

    #[test]
    fn log_alerts_do_not_advance_stage_state() {
        let mut model = aur_model();
        model.apply_event(&cloning("pkg-a"));
        model.apply_event(&build_completed("pkg-a"));
        assert_state(&model, AurStage::Finalize, StageState::Pending);
        model.apply_event(&InstallEvent::Log {
            level: LogLevel::Error,
            message: "key unknown\n".to_string(),
        });
        assert_state(&model, AurStage::Finalize, StageState::Pending);
        assert_eq!(model.aur.finalize.alerts.len(), 1);
        assert!(!model.aur.finalize.is_empty());
    }

    #[test]
    fn fail_closed_event_records_failure_note() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.apply_event(&InstallEvent::FailClosed {
            key: QuestionKey::Proceed,
            reason: "denied in test".to_string(),
        });
        assert_eq!(
            model.failure_message.as_deref(),
            Some("Proceed: denied in test")
        );
        model.finish(ChildOutcome::Failed("install failed".to_string()));
        assert_eq!(
            model.failure_message.as_deref(),
            Some("Proceed: denied in test")
        );
    }

    fn test_writer() -> AnswerWriter {
        let mut child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn true");
        let writer = AnswerWriter::from_stdin(child.stdin.take().expect("piped stdin"));
        let _ = child.wait();
        writer
    }

    #[test]
    fn runtime_prompt_stores_pending_import_key_until_answered() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.set_answer_channel(test_writer());
        model.apply_event(&import_key_event());
        let pending = model.pending_import_key.as_ref().expect("pending prompt");
        assert!(matches!(
            pending,
            Question::ImportKey { fingerprint, uid }
            if fingerprint == "ABCDEF" && uid == "Packager <pack@example.com>"
        ));
        model.answer_import_key(true);
        assert!(model.pending_import_key.is_none());
    }

    #[test]
    fn runtime_prompt_without_channel_stores_nothing() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.apply_event(&import_key_event());
        assert!(model.pending_import_key.is_none());
    }

    #[test]
    fn new_channel_drops_previous_pending_prompt() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.set_answer_channel(test_writer());
        model.apply_event(&import_key_event());
        assert!(model.pending_import_key.is_some());
        model.set_answer_channel(test_writer());
        assert!(model.pending_import_key.is_none());
        assert!(model.answer_channel.is_some());
    }

    #[test]
    fn finish_clears_pending_prompt_and_channel() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.set_answer_channel(test_writer());
        model.apply_event(&import_key_event());
        assert!(model.pending_import_key.is_some());
        model.finish(ChildOutcome::Failed("done".to_string()));
        assert!(model.pending_import_key.is_none());
        assert!(model.answer_channel.is_none());
    }

    #[test]
    fn constructor_seeds_single_target_fields() {
        let aur =
            TransactionModel::new("yay".to_string(), PackageSource::Aur, InstallKind::Install);
        assert_eq!(aur.targets, ["yay"]);
        assert_eq!(aur.aur_names, ["yay"]);
        let repo = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        assert_eq!(repo.targets, ["firefox"]);
        assert!(repo.aur_names.is_empty());
        let remove = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Remove,
        );
        assert_eq!(remove.targets, ["firefox"]);
        assert!(remove.aur_names.is_empty());
        let upgrade = TransactionModel::new(
            "system".to_string(),
            PackageSource::Repo,
            InstallKind::Upgrade,
        );
        assert_eq!(upgrade.targets, ["system"]);
        assert!(upgrade.aur_names.is_empty());
    }

    #[test]
    fn deps_stage_pending_until_bucket_fills() {
        let model = aur_model();
        assert_state(&model, AurStage::Deps, StageState::Pending);
        assert!(!model.deps_section_visible());
    }

    #[test]
    fn deps_stage_active_after_download_event() {
        let mut model = aur_model();
        model.apply_event(&retrieving(1));
        assert_state(&model, AurStage::Deps, StageState::Active);
        assert!(model.deps_section_visible());
    }

    #[test]
    fn deps_stage_active_after_install_event() {
        let mut model = aur_model();
        model.apply_event(&installed("dep1"));
        assert_state(&model, AurStage::Deps, StageState::Active);
        assert!(model.deps_section_visible());
    }

    #[test]
    fn deps_stage_done_when_phase_flips_to_artifacts() {
        let mut model = aur_model();
        model.apply_event(&retrieving(1));
        assert_state(&model, AurStage::Deps, StageState::Active);
        model.apply_event(&build_started("pkg-a"));
        assert_state(&model, AurStage::Deps, StageState::Done);
    }

    #[test]
    fn deps_stage_done_on_success() {
        let mut model = aur_model();
        model.apply_event(&retrieving(1));
        model.finish(ChildOutcome::Success);
        assert_state(&model, AurStage::Deps, StageState::Done);
    }

    #[test]
    fn deps_stage_failed_when_failure_attributed_to_deps() {
        let mut model = aur_model();
        model.apply_event(&retrieving(1));
        model.finish(ChildOutcome::Failed("deps failed".to_string()));
        assert_state(&model, AurStage::Deps, StageState::Failed);
    }

    #[test]
    fn deps_section_visible_with_repo_dep_count() {
        let mut model = aur_model();
        model.apply_event(&resolution_complete(2));
        assert!(model.deps_section_visible());
        assert_state(&model, AurStage::Deps, StageState::Pending);
    }

    #[test]
    fn deps_section_visible_for_stale_only_when_bucket_fills() {
        let mut model = aur_model();
        assert!(!model.deps_section_visible());
        model.apply_event(&hook(1, 1));
        assert!(model.deps_section_visible());
        assert_state(&model, AurStage::Deps, StageState::Active);
    }

    #[test]
    fn artifact_sections_hidden_when_build_order_empty() {
        let model = aur_model();
        assert!(!model.artifact_sections_visible());
        let mut built = aur_model();
        built.apply_event(&cloning("pkg-a"));
        assert!(built.artifact_sections_visible());
    }

    #[test]
    fn aur_overall_waypoint_ladder() {
        let mut model = aur_model();
        assert_eq!(model.overall_progress(), 0.0);
        drive_resolve_open(&mut model);
        assert_eq!(model.overall_progress(), 5.0);
        model.apply_event(&download_progress("dep1", 50, 100));
        let partial = model.overall_progress();
        assert!(partial > 5.0 && partial < 30.0);
        drive_dep_downloaded(&mut model);
        drive_dep_registered(&mut model);
        drive_dep_progress(&mut model, 100);
        assert_eq!(model.overall_progress(), 30.0);
        assert_eq!(model.aur.phase, AurPhase::Deps);
        model.apply_event(&cloning("pkg-a"));
        assert_eq!(model.overall_progress(), 30.0);
        model.apply_event(&build_started("pkg-a"));
        assert_eq!(model.aur.phase, AurPhase::Artifacts);
        assert_eq!(model.overall_progress(), 30.0);
        model.apply_event(&build_completed("pkg-a"));
        assert_eq!(model.overall_progress(), 65.0);
        drive_artifact_installed(&mut model);
        assert_eq!(model.overall_progress(), 100.0);
    }

    #[test]
    fn aur_overall_failure_freezes_per_stage() {
        let mut resolve = aur_model();
        resolve.apply_event(&resolving("yay"));
        resolve.apply_event(&dep_resolved("yay", None));
        resolve.finish(ChildOutcome::Failed("resolve failed".to_string()));
        assert_eq!(resolve.overall_progress(), 5.0);

        let mut deps = aur_model();
        drive_resolve_open(&mut deps);
        deps.apply_event(&download_progress("dep1", 50, 100));
        let before = deps.overall_progress();
        assert!(before > 5.0 && before < 30.0);
        deps.finish(ChildOutcome::Failed("deps failed".to_string()));
        assert_eq!(deps.overall_progress(), before);

        let mut build = aur_model();
        drive_resolve_with_dep(&mut build);
        drive_dep_progress(&mut build, 100);
        build.apply_event(&build_started("pkg-a"));
        build.finish(ChildOutcome::Failed("makepkg failed".to_string()));
        assert_eq!(build.overall_progress(), 30.0);

        let mut install = aur_model();
        drive_resolve_with_dep(&mut install);
        drive_dep_progress(&mut install, 100);
        drive_build_done(&mut install);
        install.apply_event(&retrieving(1));
        install.apply_event(&download_progress("pkg-a", 50, 100));
        let pre = install.overall_progress();
        assert!((65.0..100.0).contains(&pre));
        install.finish(ChildOutcome::Failed("install failed".to_string()));
        assert_eq!(install.overall_progress(), pre);
    }

    #[test]
    fn aur_overall_zero_denominators_stay_finite() {
        let model = aur_model();
        assert_eq!(model.overall_progress(), 0.0);
        let mut failed = aur_model();
        failed.apply_event(&InstallEvent::FailClosed {
            key: QuestionKey::Proceed,
            reason: "denied in test".to_string(),
        });
        assert_eq!(failed.aur.last_aur_stage, None);
        failed.finish(ChildOutcome::Failed("denied".to_string()));
        let value = failed.overall_progress();
        assert!(value.is_finite());
        assert!((0.0..=100.0).contains(&value));
    }

    #[test]
    fn aur_overall_success_stays_full() {
        let mut model = aur_model();
        drive_resolve_with_dep(&mut model);
        drive_dep_progress(&mut model, 100);
        drive_build_done(&mut model);
        drive_artifact_installed(&mut model);
        model.finish(ChildOutcome::Success);
        assert_eq!(model.overall_progress(), 100.0);
    }

    #[test]
    fn toggle_adapts_to_five_aur_stages() {
        assert_eq!(ordered_aur_stages().len(), 5);
        let mut model = aur_model();
        model.apply_event(&resolving("pkg-a"));
        model.apply_event(&dep_resolved("pkg-a", None));
        model.apply_event(&resolution_complete(1));
        model.apply_event(&retrieving(1));
        drive_build_done(&mut model);
        model.finish(ChildOutcome::Success);
        for (i, stage) in ordered_aur_stages().iter().enumerate() {
            assert_eq!(model.aur_stage_state(*stage), StageState::Done);
            model.toggle(i);
            assert!(model.expanded.contains(&i));
            model.toggle(i);
            assert!(!model.expanded.contains(&i));
        }
    }
}
