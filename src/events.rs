use serde::{Deserialize, Serialize};

use crate::tx::pacnew::{MergeAction, MergeKind};

fn default_one() -> usize {
    1
}

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
    SyncDatabases,
    StartSysupgrade,
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
        #[serde(default)]
        origin: Option<MergeOrigin>,
    },
    PacsaveCreated {
        file: String,
        #[serde(default)]
        origin: Option<MergeOrigin>,
    },
    MergeOffered {
        kind: MergeKind,
        file: String,
        from_noupgrade: bool,
        #[serde(default)]
        origin: Option<MergeOrigin>,
        #[serde(default = "default_one")]
        index: usize,
        #[serde(default = "default_one")]
        total: usize,
        hunks: Vec<MergeHunk>,
    },
    MergeResolved {
        kind: MergeKind,
        file: String,
        action: MergeAction,
    },
    RuntimePrompt {
        question: crate::question::model::Question,
    },
    FailClosed {
        key: crate::question::model::QuestionKey,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOrigin {
    pub package: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeHunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub lines: Vec<MergeLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeLine {
    Added(String),
    Removed(String),
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

pub(crate) struct DiscardSink;

impl InstallSink for DiscardSink {
    fn event(&mut self, _event: InstallEvent) {}
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
    fn pacnew_created_without_origin_deserializes_to_none() {
        let back: InstallEvent = serde_json::from_str(
            r#"{"PacnewCreated":{"from_noupgrade":false,"file":"/etc/pacman.conf"}}"#,
        )
        .expect("deserialize");
        match back {
            InstallEvent::PacnewCreated { origin, .. } => assert_eq!(origin, None),
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn pacnew_created_with_origin_round_trips() {
        let event = InstallEvent::PacnewCreated {
            from_noupgrade: true,
            file: "/etc/pacman.conf".to_string(),
            origin: Some(MergeOrigin {
                package: "pacman".to_string(),
                old_version: Some("6.0-1".to_string()),
                new_version: Some("7.0-1".to_string()),
            }),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: InstallEvent = serde_json::from_str(&json).expect("deserialize");
        match back {
            InstallEvent::PacnewCreated {
                from_noupgrade,
                file,
                origin,
            } => {
                assert!(from_noupgrade);
                assert_eq!(file, "/etc/pacman.conf");
                assert_eq!(
                    origin,
                    Some(MergeOrigin {
                        package: "pacman".to_string(),
                        old_version: Some("6.0-1".to_string()),
                        new_version: Some("7.0-1".to_string()),
                    })
                );
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn merge_offered_without_counts_defaults_to_first_of_one() {
        let back: InstallEvent = serde_json::from_str(
            r#"{"MergeOffered":{"kind":"Pacnew","file":"/etc/pacman.conf","from_noupgrade":false,"hunks":[]}}"#,
        )
        .expect("deserialize");
        match back {
            InstallEvent::MergeOffered { index, total, .. } => {
                assert_eq!(index, 1);
                assert_eq!(total, 1);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn merge_offered_with_counts_honors_explicit_values() {
        let back: InstallEvent = serde_json::from_str(
            r#"{"MergeOffered":{"kind":"Pacsave","file":"/etc/hosts","from_noupgrade":true,"index":2,"total":5,"hunks":[]}}"#,
        )
        .expect("deserialize");
        match back {
            InstallEvent::MergeOffered { index, total, .. } => {
                assert_eq!(index, 2);
                assert_eq!(total, 5);
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
