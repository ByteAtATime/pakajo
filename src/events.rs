use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
