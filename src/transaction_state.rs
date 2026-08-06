use std::collections::HashMap;

use crate::events::{InstallEvent, ProgressPhase, TransactionSummary};
use crate::install::InstallProgress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Install,
    Remove,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysupgradePhase {
    Repo,
    Aur,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoStage {
    Resolve,
    Validate,
    Download,
    Install,
    Finalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AurStage {
    Resolve,
    Build,
    Validate,
    Install,
    Finalize,
}

#[derive(Debug, Clone)]
pub struct RepoState {
    pub manifest: Option<TransactionSummary>,
    pub stage: RepoStage,
    pub download_total: usize,
    pub download_done: usize,
    pub download_bytes_total: i64,
    pub download_bytes_done: i64,
    pub download_files: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct AurState {
    pub manifest: Option<TransactionSummary>,
    pub stage: AurStage,
    pub building: Option<String>,
    pub cloning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Done,
    Active,
    Failed,
    Pending,
}

pub fn event_stage(ev: &InstallEvent) -> Option<RepoStage> {
    use InstallEvent::*;
    use RepoStage::*;
    match ev {
        ResolvingDependencies => Some(Resolve),
        CheckingConflicts
        | CheckingFileConflicts
        | CheckingIntegrity
        | CheckingDiskSpace
        | LoadingPackages
        | KeyringStart => Some(Validate),
        Progress { phase, .. } => match phase {
            ProgressPhase::Conflicts
            | ProgressPhase::Diskspace
            | ProgressPhase::Integrity
            | ProgressPhase::Load
            | ProgressPhase::Keyring => Some(Validate),
            ProgressPhase::Add
            | ProgressPhase::Upgrade
            | ProgressPhase::Downgrade
            | ProgressPhase::Reinstall
            | ProgressPhase::Remove => Some(Install),
        },
        RetrievingPackages { .. }
        | DownloadInit { .. }
        | DownloadProgress { .. }
        | DownloadRetry { .. }
        | DownloadCompleted { .. } => Some(Download),
        PackageOperation { .. } => Some(Install),
        HookRun { .. } | ScriptletInfo { .. } | TransactionDone => Some(Finalize),
        _ => None,
    }
}

pub fn aur_event_stage(ev: &InstallEvent) -> Option<AurStage> {
    use AurStage::*;
    use InstallEvent::*;
    match ev {
        ResolvingAurDependencies { .. }
        | AurDepResolved { .. }
        | ResolutionComplete { .. }
        | LayerBoundary { .. } => Some(Resolve),
        CloningRepo { .. } | BuildStarted { .. } | BuildOutput { .. } | BuildCompleted { .. } => {
            Some(Build)
        }
        LoadingPackages
        | ResolvingDependencies
        | CheckingConflicts
        | CheckingFileConflicts
        | CheckingIntegrity
        | CheckingDiskSpace
        | KeyringStart => Some(Validate),
        ProcessingChanges | PackageOperation { .. } => Some(Install),
        Progress { phase, .. } => match phase {
            ProgressPhase::Conflicts
            | ProgressPhase::Diskspace
            | ProgressPhase::Integrity
            | ProgressPhase::Load
            | ProgressPhase::Keyring => Some(Validate),
            ProgressPhase::Add
            | ProgressPhase::Upgrade
            | ProgressPhase::Downgrade
            | ProgressPhase::Reinstall
            | ProgressPhase::Remove => Some(Install),
        },
        TransactionDone | HookRun { .. } | ScriptletInfo { .. } => Some(Finalize),
        _ => None,
    }
}

pub fn cell_state(status: &InstallProgress, current_idx: usize, idx: usize) -> CellState {
    match status {
        InstallProgress::Completed => CellState::Done,
        InstallProgress::Failed(_) => {
            if idx < current_idx {
                CellState::Done
            } else if idx == current_idx {
                CellState::Failed
            } else {
                CellState::Pending
            }
        }
        _ => {
            if idx < current_idx {
                CellState::Done
            } else if idx == current_idx {
                CellState::Active
            } else {
                CellState::Pending
            }
        }
    }
}

pub fn apply_repo_counters(state: &mut RepoState, ev: &InstallEvent) {
    match ev {
        InstallEvent::RetrievingPackages { num, total_bytes } => {
            state.download_total = *num;
            state.download_done = 0;
            state.download_bytes_total = *total_bytes;
            state.download_bytes_done = 0;
            state.download_files.clear();
        }
        InstallEvent::DownloadProgress {
            filename,
            downloaded,
            ..
        } => {
            let prev = state
                .download_files
                .insert(filename.clone(), *downloaded)
                .unwrap_or(0);
            state.download_bytes_done += *downloaded - prev;
        }
        InstallEvent::DownloadRetry { filename, resume } => {
            if !*resume && let Some(prev) = state.download_files.remove(filename) {
                state.download_bytes_done -= prev;
            }
        }
        InstallEvent::DownloadCompleted { .. } => {
            state.download_done += 1;
        }
        _ => {}
    }
}

pub fn ordered_stages(kind: InstallKind) -> Vec<RepoStage> {
    use RepoStage::*;
    match kind {
        InstallKind::Install | InstallKind::Upgrade => {
            vec![Resolve, Validate, Download, Install, Finalize]
        }
        InstallKind::Remove => vec![Resolve, Validate, Install, Finalize],
    }
}

pub fn ordered_aur_stages() -> &'static [AurStage] {
    use AurStage::*;
    &[Resolve, Build, Validate, Install, Finalize]
}

#[derive(Debug, Clone, PartialEq)]
pub enum NextInstallState {
    ContinueAur { targets: Vec<String> },
    Completed,
    Cancelled,
    Failed { message: String },
}

pub fn classify_outcome(
    outcome: &crate::install::ChildOutcome,
    active_phase: Option<SysupgradePhase>,
    aur_targets: &[String],
) -> NextInstallState {
    use crate::install::ChildOutcome;
    if matches!(outcome, ChildOutcome::Success)
        && active_phase == Some(SysupgradePhase::Repo)
        && !aur_targets.is_empty()
    {
        return NextInstallState::ContinueAur {
            targets: aur_targets.to_vec(),
        };
    }
    match outcome {
        ChildOutcome::Success => NextInstallState::Completed,
        ChildOutcome::Dismissed => NextInstallState::Cancelled,
        ChildOutcome::NotFound => NextInstallState::Failed {
            message: "install child not found".to_string(),
        },
        ChildOutcome::Failed(message) => NextInstallState::Failed {
            message: message.clone(),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysupgradePage {
    Updates,
    Resolve,
    PkgbuildReview,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

pub fn next_sysupgrade_step(
    from: SysupgradePage,
    dir: Direction,
    has_resolve: bool,
    has_diffs: bool,
) -> SysupgradePage {
    use Direction::*;
    use SysupgradePage::*;
    match (from, dir) {
        (Updates, Forward) => {
            if has_resolve {
                Resolve
            } else if has_diffs {
                PkgbuildReview
            } else {
                Confirm
            }
        }
        (Resolve, Forward) => {
            if has_diffs {
                PkgbuildReview
            } else {
                Confirm
            }
        }
        (Resolve, Backward) => Updates,
        (PkgbuildReview, Forward) => Confirm,
        (PkgbuildReview, Backward) => {
            if has_resolve {
                Resolve
            } else {
                Updates
            }
        }
        (Confirm, Backward) => {
            if has_diffs {
                PkgbuildReview
            } else if has_resolve {
                Resolve
            } else {
                Updates
            }
        }
        _ => from,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::events::{DownloadResult, InstallEvent, PackageOp, ProgressPhase};
    use crate::install::InstallProgress;
    use crate::install::ChildOutcome;

    #[test]
    fn repo_resolving_dependencies_is_resolve() {
        assert_eq!(
            event_stage(&InstallEvent::ResolvingDependencies),
            Some(RepoStage::Resolve)
        );
    }

    #[test]
    fn repo_checking_conflicts_is_validate() {
        assert_eq!(
            event_stage(&InstallEvent::CheckingConflicts),
            Some(RepoStage::Validate)
        );
    }

    #[test]
    fn repo_download_progress_is_download() {
        let ev = InstallEvent::DownloadProgress {
            filename: "foo.pkg.tar.zst".to_string(),
            downloaded: 100,
            total: 1000,
        };
        assert_eq!(event_stage(&ev), Some(RepoStage::Download));
    }

    #[test]
    fn repo_retrieving_packages_is_download() {
        let ev = InstallEvent::RetrievingPackages {
            num: 3,
            total_bytes: 5000,
        };
        assert_eq!(event_stage(&ev), Some(RepoStage::Download));
    }

    #[test]
    fn repo_package_operation_is_install() {
        let ev = InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: "foo".to_string(),
            new_version: Some("1.0".to_string()),
            old_version: None,
        };
        assert_eq!(event_stage(&ev), Some(RepoStage::Install));
    }

    #[test]
    fn repo_transaction_done_is_finalize() {
        assert_eq!(
            event_stage(&InstallEvent::TransactionDone),
            Some(RepoStage::Finalize)
        );
    }

    #[test]
    fn repo_progress_upgrade_is_install() {
        let ev = InstallEvent::Progress {
            phase: ProgressPhase::Upgrade,
            package: "foo".to_string(),
            percent: 50,
            current: 1,
            total: 2,
        };
        assert_eq!(event_stage(&ev), Some(RepoStage::Install));
    }

    #[test]
    fn repo_progress_integrity_is_validate() {
        let ev = InstallEvent::Progress {
            phase: ProgressPhase::Integrity,
            package: "foo".to_string(),
            percent: 0,
            current: 0,
            total: 0,
        };
        assert_eq!(event_stage(&ev), Some(RepoStage::Validate));
    }

    #[test]
    fn aur_resolving_dependencies_is_resolve() {
        let ev = InstallEvent::ResolvingAurDependencies {
            target: "foo".to_string(),
        };
        assert_eq!(aur_event_stage(&ev), Some(AurStage::Resolve));
    }

    #[test]
    fn aur_cloning_repo_is_build() {
        let ev = InstallEvent::CloningRepo {
            package: "foo".to_string(),
        };
        assert_eq!(aur_event_stage(&ev), Some(AurStage::Build));
    }

    #[test]
    fn aur_build_started_is_build() {
        let ev = InstallEvent::BuildStarted {
            package: "foo".to_string(),
        };
        assert_eq!(aur_event_stage(&ev), Some(AurStage::Build));
    }

    #[test]
    fn aur_loading_packages_is_validate() {
        assert_eq!(
            aur_event_stage(&InstallEvent::LoadingPackages),
            Some(AurStage::Validate)
        );
    }

    #[test]
    fn aur_package_operation_is_install() {
        let ev = InstallEvent::PackageOperation {
            operation: PackageOp::Upgrade,
            package: "foo".to_string(),
            new_version: Some("2.0".to_string()),
            old_version: Some("1.0".to_string()),
        };
        assert_eq!(aur_event_stage(&ev), Some(AurStage::Install));
    }

    #[test]
    fn aur_transaction_done_is_finalize() {
        assert_eq!(
            aur_event_stage(&InstallEvent::TransactionDone),
            Some(AurStage::Finalize)
        );
    }

    fn fresh_repo_state() -> RepoState {
        RepoState {
            manifest: None,
            stage: RepoStage::Resolve,
            download_total: 0,
            download_done: 0,
            download_bytes_total: 0,
            download_bytes_done: 0,
            download_files: HashMap::new(),
        }
    }

    #[test]
    fn cell_state_completed_is_done_for_any_index() {
        assert_eq!(cell_state(&InstallProgress::Completed, 0, 0), CellState::Done);
        assert_eq!(cell_state(&InstallProgress::Completed, 2, 5), CellState::Done);
    }

    #[test]
    fn cell_state_failed_partitions_by_current_index() {
        let status = InstallProgress::Failed("boom".to_string());
        assert_eq!(cell_state(&status, 2, 0), CellState::Done);
        assert_eq!(cell_state(&status, 2, 2), CellState::Failed);
        assert_eq!(cell_state(&status, 2, 3), CellState::Pending);
    }

    #[test]
    fn cell_state_running_partitions_by_current_index() {
        assert_eq!(cell_state(&InstallProgress::Running, 2, 0), CellState::Done);
        assert_eq!(cell_state(&InstallProgress::Running, 2, 2), CellState::Active);
        assert_eq!(cell_state(&InstallProgress::Running, 2, 3), CellState::Pending);
    }

    #[test]
    fn apply_repo_retrieving_packages_sets_totals() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 3,
                total_bytes: 5000,
            },
        );
        assert_eq!(state.download_total, 3);
        assert_eq!(state.download_done, 0);
        assert_eq!(state.download_bytes_total, 5000);
        assert_eq!(state.download_bytes_done, 0);
        assert!(state.download_files.is_empty());
    }

    #[test]
    fn apply_repo_retrieving_packages_clears_existing_map() {
        let mut state = fresh_repo_state();
        state.download_files.insert("stale".to_string(), 999);
        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 1,
                total_bytes: 10,
            },
        );
        assert!(state.download_files.is_empty());
    }

    #[test]
    fn apply_repo_download_progress_tracks_delta() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 100,
                total: 200,
            },
        );
        assert_eq!(state.download_files.get("a"), Some(&100));
        assert_eq!(state.download_bytes_done, 100);
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 150,
                total: 200,
            },
        );
        assert_eq!(state.download_files.get("a"), Some(&150));
        assert_eq!(state.download_bytes_done, 150);
    }

    #[test]
    fn apply_repo_download_retry_full_removes_entry() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 100,
                total: 200,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadRetry {
                filename: "a".to_string(),
                resume: false,
            },
        );
        assert!(state.download_files.is_empty());
        assert_eq!(state.download_bytes_done, 0);
    }

    #[test]
    fn apply_repo_download_retry_resumable_is_noop() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 100,
                total: 200,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadRetry {
                filename: "a".to_string(),
                resume: true,
            },
        );
        assert_eq!(state.download_files.get("a"), Some(&100));
        assert_eq!(state.download_bytes_done, 100);
    }

    #[test]
    fn apply_repo_download_completed_increments_done() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadCompleted {
                filename: "a".to_string(),
                total: 200,
                result: DownloadResult::Success,
            },
        );
        assert_eq!(state.download_done, 1);
    }

    #[test]
    fn apply_repo_unrelated_event_is_noop() {
        let mut state = fresh_repo_state();
        apply_repo_counters(&mut state, &InstallEvent::ResolvingDependencies);
        assert_eq!(state.download_total, 0);
        assert_eq!(state.download_done, 0);
        assert_eq!(state.download_bytes_total, 0);
        assert_eq!(state.download_bytes_done, 0);
        assert!(state.download_files.is_empty());
    }

    #[test]
    fn ordered_stages_install_includes_download() {
        assert_eq!(
            ordered_stages(InstallKind::Install),
            vec![
                RepoStage::Resolve,
                RepoStage::Validate,
                RepoStage::Download,
                RepoStage::Install,
                RepoStage::Finalize,
            ]
        );
    }

    #[test]
    fn ordered_stages_upgrade_matches_install() {
        assert_eq!(
            ordered_stages(InstallKind::Upgrade),
            ordered_stages(InstallKind::Install)
        );
    }

    #[test]
    fn ordered_stages_remove_skips_download() {
        assert_eq!(
            ordered_stages(InstallKind::Remove),
            vec![
                RepoStage::Resolve,
                RepoStage::Validate,
                RepoStage::Install,
                RepoStage::Finalize,
            ]
        );
    }

    #[test]
    fn ordered_aur_stages_lists_build() {
        assert_eq!(
            ordered_aur_stages(),
            &[
                AurStage::Resolve,
                AurStage::Build,
                AurStage::Validate,
                AurStage::Install,
                AurStage::Finalize,
            ]
        );
    }

    #[test]
    fn classify_success_without_phase_completes() {
        assert_eq!(
            classify_outcome(&ChildOutcome::Success, None, &[]),
            NextInstallState::Completed
        );
    }

    #[test]
    fn classify_success_repo_phase_with_targets_continues_aur() {
        let targets = vec!["foo".to_string(), "bar".to_string()];
        assert_eq!(
            classify_outcome(
                &ChildOutcome::Success,
                Some(SysupgradePhase::Repo),
                &targets
            ),
            NextInstallState::ContinueAur { targets }
        );
    }

    #[test]
    fn classify_success_repo_phase_with_empty_targets_completes() {
        assert_eq!(
            classify_outcome(&ChildOutcome::Success, Some(SysupgradePhase::Repo), &[]),
            NextInstallState::Completed
        );
    }

    #[test]
    fn classify_dismissed_cancels() {
        assert_eq!(
            classify_outcome(&ChildOutcome::Dismissed, None, &[]),
            NextInstallState::Cancelled
        );
    }

    #[test]
    fn classify_not_found_fails() {
        assert_eq!(
            classify_outcome(&ChildOutcome::NotFound, None, &[]),
            NextInstallState::Failed {
                message: "install child not found".to_string()
            }
        );
    }

    #[test]
    fn classify_failed_propagates_message() {
        assert_eq!(
            classify_outcome(&ChildOutcome::Failed("msg".to_string()), None, &[]),
            NextInstallState::Failed {
                message: "msg".to_string()
            }
        );
    }

    #[test]
    fn next_sysupgrade_step_updates_forward_skips_to_confirm_without_steps() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Updates, Direction::Forward, false, false),
            SysupgradePage::Confirm
        );
    }

    #[test]
    fn next_sysupgrade_step_updates_forward_without_resolve_enters_pkgbuild_review() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Updates, Direction::Forward, false, true),
            SysupgradePage::PkgbuildReview
        );
    }

    #[test]
    fn next_sysupgrade_step_updates_forward_with_resolve_enters_resolve() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Updates, Direction::Forward, true, true),
            SysupgradePage::Resolve
        );
    }

    #[test]
    fn next_sysupgrade_step_resolve_forward_enters_pkgbuild_review_with_diffs() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Resolve, Direction::Forward, true, true),
            SysupgradePage::PkgbuildReview
        );
    }

    #[test]
    fn next_sysupgrade_step_resolve_forward_skips_to_confirm_without_diffs() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Resolve, Direction::Forward, true, false),
            SysupgradePage::Confirm
        );
    }

    #[test]
    fn next_sysupgrade_step_resolve_backward_returns_updates() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Resolve, Direction::Backward, true, true),
            SysupgradePage::Updates
        );
    }

    #[test]
    fn next_sysupgrade_step_pkgbuild_review_forward_enters_confirm() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::PkgbuildReview, Direction::Forward, true, true),
            SysupgradePage::Confirm
        );
    }

    #[test]
    fn next_sysupgrade_step_pkgbuild_review_backward_returns_resolve_when_present() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::PkgbuildReview, Direction::Backward, true, true),
            SysupgradePage::Resolve
        );
    }

    #[test]
    fn next_sysupgrade_step_pkgbuild_review_backward_skips_to_updates_without_resolve() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::PkgbuildReview, Direction::Backward, false, true),
            SysupgradePage::Updates
        );
    }

    #[test]
    fn next_sysupgrade_step_confirm_backward_returns_pkgbuild_review_with_diffs() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Confirm, Direction::Backward, true, true),
            SysupgradePage::PkgbuildReview
        );
    }

    #[test]
    fn next_sysupgrade_step_confirm_backward_returns_resolve_without_diffs() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Confirm, Direction::Backward, true, false),
            SysupgradePage::Resolve
        );
    }

    #[test]
    fn next_sysupgrade_step_confirm_backward_skips_to_updates_without_steps() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Confirm, Direction::Backward, false, false),
            SysupgradePage::Updates
        );
    }

    #[test]
    fn next_sysupgrade_step_confirm_forward_returns_from_unchanged() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Confirm, Direction::Forward, true, true),
            SysupgradePage::Confirm
        );
    }

    #[test]
    fn next_sysupgrade_step_updates_backward_returns_from_unchanged() {
        assert_eq!(
            next_sysupgrade_step(SysupgradePage::Updates, Direction::Backward, true, true),
            SysupgradePage::Updates
        );
    }
}
