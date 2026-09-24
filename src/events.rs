use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkgbuildReviewEntry {
    pub name: String,
    pub pkgbase: String,
    pub is_new: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallEvent {
    ResolvingDependencies,
    CheckingConflicts,
    CheckingDependencies,
    CheckingFileConflicts,
    CheckingIntegrity,
    CheckingDiskSpace,
    LoadingPackages,
    KeyringStart,
    RetrievingPackages {
        num: usize,
        total_bytes: i64,
    },
    PackageOperation {
        operation: PackageOp,
        package: String,
        new_version: Option<String>,
        old_version: Option<String>,
    },
    DownloadInit {
        filename: String,
        optional: bool,
    },
    DownloadProgress {
        filename: String,
        downloaded: i64,
        total: i64,
    },
    DownloadRetry {
        filename: String,
        resume: bool,
    },
    DownloadCompleted {
        filename: String,
        total: i64,
        result: DownloadResult,
    },
    Progress {
        phase: ProgressPhase,
        package: String,
        percent: i32,
        current: usize,
        total: usize,
    },
    HookStart {
        pre: bool,
    },
    HookRun {
        position: usize,
        total: usize,
        name: String,
        desc: Option<String>,
    },
    ScriptletInfo {
        line: String,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    TransactionDone,
    ProcessingChanges,
    TransactionSummary(TransactionSummary),
    ResolvingAurDependencies {
        target: String,
    },
    AurDepResolved {
        package: String,
        repo: Option<String>,
        #[serde(default)]
        version: Option<String>,
    },
    ResolutionComplete {
        aur_packages: usize,
        repo_deps: usize,
    },
    CloningRepo {
        package: String,
    },
    BuildStarted {
        package: String,
    },
    BuildOutput {
        package: String,
        line: String,
    },
    BuildCompleted {
        package: String,
        artifacts: Vec<String>,
        #[serde(default)]
        version: Option<String>,
    },
    SysupgradeAurCandidates {
        candidates: Vec<crate::upgrade::AurUpgradeCandidate>,
    },
    PkgbuildReviewStarted {
        packages: Vec<PkgbuildReviewEntry>,
    },
    PkgbuildReviewAccepted {
        packages: Vec<String>,
    },
    PkgbuildAllUpToDate {
        packages: Vec<String>,
    },
    WaitingForDatabaseLock,
    ResolveDepsDone,
    CheckDepsDone,
    InterConflictsDone,
    FileConflictsDone,
    IntegrityDone,
    LoadDone,
    DiskSpaceDone,
    KeyringDone,
    KeyDownloadStart,
    KeyDownloadDone,
    RetrieveStart,
    RetrieveDone,
    RetrieveFailed,
    PkgRetrieveDone {
        num: usize,
        total_bytes: i64,
    },
    PkgRetrieveFailed {
        num: usize,
        total_bytes: i64,
    },
    PackageOperationEnd {
        operation: PackageOp,
        package: String,
    },
    HookDone {
        pre: bool,
    },
    HookRunDone,
    OptDepRemoval {
        package: String,
        optdep: String,
    },
    DatabaseMissing {
        dbname: String,
    },
    PacnewCreated {
        from_noupgrade: bool,
        file: String,
    },
    PacsaveCreated {
        file: String,
    },
    RuntimePrompt {
        question: crate::question::model::Question,
    },
    FailClosed {
        key: crate::question::model::QuestionKey,
        reason: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TransactionSummary {
    pub packages: Vec<SummaryPackage>,
    pub total_download_size: i64,
    pub total_installed_size: i64,
    #[serde(default)]
    pub total_removed_size: i64,
}

impl TransactionSummary {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
            && self.total_download_size == 0
            && self.total_installed_size == 0
            && self.total_removed_size == 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryPackage {
    pub name: String,
    pub repository: Option<String>,
    pub new_version: String,
    pub old_version: Option<String>,
    pub download_size: i64,
    pub installed_size: i64,
    #[serde(default)]
    pub old_installed_size: i64,
    #[serde(default)]
    pub is_removal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SummaryAction {
    Install,
    Upgrade,
    Downgrade,
    Reinstall,
    Remove,
}

pub fn classify_action(pkg: &SummaryPackage) -> SummaryAction {
    if pkg.is_removal {
        return SummaryAction::Remove;
    }
    let Some(old) = pkg.old_version.as_deref() else {
        return SummaryAction::Install;
    };
    match alpm::vercmp(pkg.new_version.clone(), old.to_string()) {
        std::cmp::Ordering::Greater => SummaryAction::Upgrade,
        std::cmp::Ordering::Less => SummaryAction::Downgrade,
        std::cmp::Ordering::Equal => SummaryAction::Reinstall,
    }
}

pub fn target_version(pkg: &SummaryPackage) -> &str {
    if pkg.is_removal {
        return pkg.old_version.as_deref().unwrap_or("");
    }
    &pkg.new_version
}

pub fn summary_fingerprint(
    summary: &TransactionSummary,
) -> std::collections::BTreeSet<(String, SummaryAction, String)> {
    summary
        .packages
        .iter()
        .map(|pkg| {
            (
                pkg.name.clone(),
                classify_action(pkg),
                target_version(pkg).to_string(),
            )
        })
        .collect()
}

pub fn summaries_match(prev: &TransactionSummary, cur: &TransactionSummary) -> bool {
    summary_fingerprint(prev) == summary_fingerprint(cur)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PackageOp {
    Install,
    Upgrade,
    Reinstall,
    Downgrade,
    Remove,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DownloadResult {
    Success,
    UpToDate,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressPhase {
    Add,
    Upgrade,
    Downgrade,
    Reinstall,
    Remove,
    Conflicts,
    Diskspace,
    Integrity,
    Load,
    Keyring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Error,
    Warning,
    Debug,
}

pub trait InstallSink {
    fn event(&mut self, event: InstallEvent);
}

pub fn read_event_stream<R: std::io::BufRead, S: InstallSink + ?Sized>(reader: R, sink: &mut S) {
    for line in reader.lines() {
        match line {
            Ok(text) => {
                if let Ok(event) = serde_json::from_str::<InstallEvent>(&text) {
                    sink.event(event);
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_prompt_round_trips_json() {
        let event = InstallEvent::RuntimePrompt {
            question: crate::question::model::Question::ImportKey {
                fingerprint: "ABCDEF".to_string(),
                uid: "Packager <pack@example.com>".to_string(),
            },
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: InstallEvent = serde_json::from_str(&json).expect("deserialize");
        match back {
            InstallEvent::RuntimePrompt { question } => {
                assert_eq!(
                    question,
                    crate::question::model::Question::ImportKey {
                        fingerprint: "ABCDEF".to_string(),
                        uid: "Packager <pack@example.com>".to_string(),
                    }
                );
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn sysupgrade_aur_candidates_round_trips_json() {
        let event = InstallEvent::SysupgradeAurCandidates {
            candidates: vec![crate::upgrade::AurUpgradeCandidate {
                name: "foo".to_string(),
                local_version: "1.0".to_string(),
                remote_version: "1.1".to_string(),
                package_base: "foo".to_string(),
            }],
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: InstallEvent = serde_json::from_str(&json).expect("deserialize");
        match back {
            InstallEvent::SysupgradeAurCandidates { candidates } => {
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0].name, "foo");
                assert_eq!(candidates[0].remote_version, "1.1");
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }
}

#[cfg(test)]
mod staleness_tests {
    use super::*;

    fn pkg(name: &str, old: Option<&str>, new: &str, is_removal: bool) -> SummaryPackage {
        SummaryPackage {
            name: name.to_string(),
            repository: None,
            new_version: new.to_string(),
            old_version: old.map(str::to_string),
            download_size: 0,
            installed_size: 0,
            old_installed_size: 0,
            is_removal,
        }
    }

    fn summary(packages: Vec<SummaryPackage>) -> TransactionSummary {
        TransactionSummary {
            packages,
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    #[test]
    fn identical_summaries_match() {
        let prev = summary(vec![
            pkg("alpha", None, "1.0", false),
            pkg("beta", Some("2.0"), "2.1", false),
        ]);
        let cur = summary(vec![
            pkg("alpha", None, "1.0", false),
            pkg("beta", Some("2.0"), "2.1", false),
        ]);
        assert!(summaries_match(&prev, &cur));
    }

    #[test]
    fn reordered_summaries_match() {
        let prev = summary(vec![
            pkg("alpha", None, "1.0", false),
            pkg("beta", Some("2.0"), "2.1", false),
        ]);
        let cur = summary(vec![
            pkg("beta", Some("2.0"), "2.1", false),
            pkg("alpha", None, "1.0", false),
        ]);
        assert!(summaries_match(&prev, &cur));
    }

    #[test]
    fn changed_target_version_does_not_match() {
        let prev = summary(vec![pkg("alpha", Some("1.0"), "2.0", false)]);
        let cur = summary(vec![pkg("alpha", Some("1.0"), "2.1", false)]);
        assert!(!summaries_match(&prev, &cur));
    }

    #[test]
    fn added_package_does_not_match() {
        let prev = summary(vec![pkg("alpha", None, "1.0", false)]);
        let cur = summary(vec![
            pkg("alpha", None, "1.0", false),
            pkg("beta", None, "3.0", false),
        ]);
        assert!(!summaries_match(&prev, &cur));
    }

    #[test]
    fn removed_package_does_not_match() {
        let prev = summary(vec![
            pkg("alpha", None, "1.0", false),
            pkg("beta", None, "3.0", false),
        ]);
        let cur = summary(vec![pkg("alpha", None, "1.0", false)]);
        assert!(!summaries_match(&prev, &cur));
    }

    #[test]
    fn size_changes_ignored_when_operation_identical() {
        let prev = TransactionSummary {
            packages: vec![pkg("alpha", Some("1.0"), "2.0", false)],
            total_download_size: 100,
            total_installed_size: 500,
            total_removed_size: 0,
        };
        let cur = TransactionSummary {
            packages: vec![pkg("alpha", Some("1.0"), "2.0", false)],
            total_download_size: 999,
            total_installed_size: 9999,
            total_removed_size: 50,
        };
        assert!(summaries_match(&prev, &cur));
    }

    #[test]
    fn action_change_does_not_match() {
        let prev = summary(vec![pkg("alpha", None, "1.0", false)]);
        let cur = summary(vec![pkg("alpha", Some("1.0"), "", true)]);
        assert!(!summaries_match(&prev, &cur));
    }

    #[test]
    fn upgrade_vs_downgrade_does_not_match() {
        let prev = summary(vec![pkg("alpha", Some("1.0"), "2.0", false)]);
        let cur = summary(vec![pkg("alpha", Some("1.0"), "0.9", false)]);
        assert!(!summaries_match(&prev, &cur));
    }

    #[test]
    fn removal_version_change_does_not_match() {
        let prev = summary(vec![pkg("alpha", Some("1.0"), "", true)]);
        let cur = summary(vec![pkg("alpha", Some("2.0"), "", true)]);
        assert!(!summaries_match(&prev, &cur));
    }
}
