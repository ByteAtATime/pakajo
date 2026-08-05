use crate::events::{InstallEvent, ProgressPhase};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{InstallEvent, PackageOp, ProgressPhase};

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
}
