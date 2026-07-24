use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AurDepSource {
    Repo,
    Aur,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallEvent {
    ResolvingDependencies,
    CheckingConflicts,
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
    TransactionSummary(TransactionSummary),
    ResolvingAurDependencies {
        target: String,
    },
    AurDepResolved {
        package: String,
        source: AurDepSource,
    },
    ResolutionComplete {
        layers: usize,
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
    LayerBoundary {
        layer: usize,
        total: usize,
    },
    SysupgradeAurCandidates {
        candidates: Vec<crate::upgrade::AurUpgradeCandidate>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionSummary {
    pub packages: Vec<SummaryPackage>,
    pub total_download_size: i64,
    pub total_installed_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPackage {
    pub name: String,
    pub repository: Option<String>,
    pub new_version: String,
    pub old_version: Option<String>,
    pub download_size: i64,
    pub installed_size: i64,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
