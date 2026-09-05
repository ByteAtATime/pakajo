use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::Instant;

use crate::events::{InstallEvent, PackageOp, ProgressPhase, TransactionSummary};

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

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadFile {
    pub downloaded: i64,
    pub total: i64,
    pub completed: bool,
    pub rate: f64,
    pub sync_time: Option<std::time::Instant>,
    pub sync_done: i64,
}

#[derive(Debug, Clone)]
pub struct InstallPackage {
    pub operation: PackageOp,
    pub new_version: Option<String>,
    pub old_version: Option<String>,
    pub percent: f32,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveState {
    pub started: bool,
    pub checking: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ValidateState {
    pub checks: u8,
    pub label: &'static str,
}

impl Default for ValidateState {
    fn default() -> Self {
        Self {
            checks: 0,
            label: "Preparing...",
        }
    }
}

impl ValidateState {
    pub fn count(&self) -> usize {
        self.checks.count_ones() as usize
    }
}

#[derive(Debug, Clone, Default)]
pub struct DownloadState {
    pub total: usize,
    pub done: usize,
    pub bytes_total: i64,
    pub bytes_done: i64,
    pub files: HashMap<String, DownloadFile>,
    pub order: Vec<String>,
    pub rate: f64,
    pub sync_time: Option<Instant>,
    pub sync_done: i64,
}

impl DownloadState {
    pub fn queued(&self) -> usize {
        let active = self.files.values().filter(|f| !f.completed).count();
        self.total.saturating_sub(self.done).saturating_sub(active)
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallState {
    pub order: Vec<String>,
    pub packages: HashMap<String, InstallPackage>,
}

#[derive(Debug, Clone, Default)]
pub struct FinalizeState {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RepoState {
    pub manifest: Option<TransactionSummary>,
    pub resolve: ResolveState,
    pub validate: ValidateState,
    pub download: DownloadState,
    pub install: InstallState,
    pub finalize: FinalizeState,
}

impl RepoState {
    pub fn resolve_step(&self) -> usize {
        if self.manifest.is_some() {
            return 3;
        }
        if self.resolve.checking {
            return 2;
        }
        if self.resolve.started {
            return 1;
        }
        0
    }
}

pub const VALIDATE_TOTAL: usize = VALIDATE_LABELS.len();

const VALIDATE_LABELS: [&str; 6] = [
    "Looking for conflicting packages...",
    "Checking dependencies...",
    "Checking keys in keyring...",
    "Checking package integrity...",
    "Checking for file conflicts...",
    "Checking available disk space...",
];

fn validate_bit(ev: &InstallEvent) -> Option<u8> {
    use InstallEvent::*;
    match ev {
        CheckingConflicts => Some(0),
        CheckingDependencies => Some(1),
        KeyringStart => Some(2),
        CheckingIntegrity => Some(3),
        CheckingFileConflicts => Some(4),
        CheckingDiskSpace => Some(5),
        _ => None,
    }
}

fn track_validate_check(state: &mut ValidateState, ev: &InstallEvent) {
    if let Some(bit) = validate_bit(ev) {
        state.checks |= 1 << bit;
        state.label = VALIDATE_LABELS[bit as usize];
    }
}

fn is_install_phase(phase: &ProgressPhase) -> bool {
    use ProgressPhase::*;
    matches!(phase, Add | Upgrade | Downgrade | Reinstall | Remove)
}

pub fn event_stage(ev: &InstallEvent) -> Option<RepoStage> {
    use InstallEvent::*;
    use RepoStage::*;
    match ev {
        ResolvingDependencies => Some(Resolve),
        CheckingConflicts
        | CheckingDependencies
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

fn apply_resolve(state: &mut ResolveState, ev: &InstallEvent) {
    match ev {
        InstallEvent::ResolvingDependencies | InstallEvent::ResolvingAurDependencies { .. } => {
            state.started = true;
        }
        _ => {}
    }
}

fn apply_install(state: &mut InstallState, ev: &InstallEvent) {
    match ev {
        InstallEvent::PackageOperation {
            operation,
            package,
            new_version,
            old_version,
        } => {
            if !state.packages.contains_key(package) {
                state.order.push(package.clone());
            }
            state.packages.insert(
                package.clone(),
                InstallPackage {
                    operation: *operation,
                    new_version: new_version.clone(),
                    old_version: old_version.clone(),
                    percent: 0.0,
                    completed: false,
                },
            );
        }
        InstallEvent::Progress {
            phase,
            package,
            percent,
            ..
        } if is_install_phase(phase) => {
            if let Some(entry) = state.packages.get_mut(package) {
                entry.percent = (*percent as f32).clamp(0.0, 100.0);
                entry.completed = *percent >= 100;
            }
        }
        _ => {}
    }
}

fn apply_finalize(state: &mut FinalizeState, ev: &InstallEvent) {
    match ev {
        InstallEvent::HookRun {
            position,
            total,
            name,
            desc,
        } => {
            let label = desc.as_deref().unwrap_or(name);
            state.lines.push(format!("({position}/{total}) {label}"));
        }
        InstallEvent::ScriptletInfo { line } => {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                state.lines.push(trimmed.to_string());
            }
        }
        _ => {}
    }
}

fn apply_download(state: &mut DownloadState, ev: &InstallEvent) {
    match ev {
        InstallEvent::RetrievingPackages { num, total_bytes } => {
            state.total = *num;
            state.done = 0;
            state.bytes_total = *total_bytes;
            state.bytes_done = 0;
            state.files.clear();
            state.order.clear();
            state.rate = 0.0;
            state.sync_time = None;
            state.sync_done = 0;
        }
        InstallEvent::DownloadInit { filename, .. } => {
            ensure_download_file(state, filename);
        }
        InstallEvent::DownloadProgress {
            filename,
            downloaded,
            total,
        } => {
            let entry = ensure_download_file(state, filename);
            let prev = entry.downloaded;
            entry.downloaded = *downloaded;
            entry.total = *total;
            update_file_rate(entry);
            state.bytes_done += *downloaded - prev;
            update_download_rate(state);
        }
        InstallEvent::DownloadRetry { filename, resume } => {
            if !*resume && let Some(f) = state.files.get_mut(filename) {
                state.bytes_done -= f.downloaded;
                f.downloaded = 0;
                f.rate = 0.0;
                f.sync_time = None;
                f.sync_done = 0;
            }
        }
        InstallEvent::DownloadCompleted {
            filename, total, ..
        } => {
            let f = ensure_download_file(state, filename);
            let already_completed = f.completed;
            let prev_downloaded = f.downloaded;
            if !already_completed {
                f.downloaded = *total;
                f.total = *total;
                f.completed = true;
            }
            if !already_completed {
                state.bytes_done += *total - prev_downloaded;
            }
            state.done += 1;
        }
        _ => {}
    }
}

pub fn apply_repo_counters(state: &mut RepoState, ev: &InstallEvent) {
    match ev {
        InstallEvent::ResolvingDependencies | InstallEvent::ResolvingAurDependencies { .. } => {
            apply_resolve(&mut state.resolve, ev);
        }
        InstallEvent::CheckingConflicts
        | InstallEvent::CheckingDependencies
        | InstallEvent::CheckingFileConflicts
        | InstallEvent::AurDepResolved { .. }
        | InstallEvent::ResolutionComplete { .. } => {
            state.resolve.started = true;
            state.resolve.checking = true;
            track_validate_check(&mut state.validate, ev);
        }
        InstallEvent::CheckingIntegrity
        | InstallEvent::CheckingDiskSpace
        | InstallEvent::KeyringStart => {
            track_validate_check(&mut state.validate, ev);
        }
        InstallEvent::TransactionSummary(summary) => {
            state.resolve.started = true;
            state.resolve.checking = true;
            state.manifest = Some(summary.clone());
        }
        InstallEvent::PackageOperation { .. } => {
            apply_install(&mut state.install, ev);
        }
        InstallEvent::Progress { phase, .. } if is_install_phase(phase) => {
            apply_install(&mut state.install, ev);
        }
        InstallEvent::HookRun { .. } | InstallEvent::ScriptletInfo { .. } => {
            apply_finalize(&mut state.finalize, ev);
        }
        InstallEvent::RetrievingPackages { .. }
        | InstallEvent::DownloadInit { .. }
        | InstallEvent::DownloadProgress { .. }
        | InstallEvent::DownloadRetry { .. }
        | InstallEvent::DownloadCompleted { .. } => {
            apply_download(&mut state.download, ev);
        }
        _ => {}
    }
}

const DOWNLOAD_RATE_SAMPLE_MS: u128 = 200;

fn update_download_rate(state: &mut DownloadState) {
    let now = Instant::now();
    let sync_time = match state.sync_time {
        Some(t) => t,
        None => {
            state.sync_time = Some(now);
            state.sync_done = state.bytes_done;
            return;
        }
    };
    let timediff = now.duration_since(sync_time).as_millis();
    if timediff < DOWNLOAD_RATE_SAMPLE_MS {
        return;
    }
    let chunk = (state.bytes_done - state.sync_done).max(0);
    state.sync_done = state.bytes_done;
    state.sync_time = Some(now);
    let chunk_rate = chunk as f64 * 1000.0 / timediff as f64;
    state.rate = (chunk_rate + 2.0 * state.rate) / 3.0;
}

fn update_file_rate(file: &mut DownloadFile) {
    let now = Instant::now();
    let sync_time = match file.sync_time {
        Some(t) => t,
        None => {
            file.sync_time = Some(now);
            file.sync_done = file.downloaded;
            return;
        }
    };
    let timediff = now.duration_since(sync_time).as_millis();
    if timediff < DOWNLOAD_RATE_SAMPLE_MS {
        return;
    }
    let chunk = (file.downloaded - file.sync_done).max(0);
    file.sync_done = file.downloaded;
    file.sync_time = Some(now);
    let chunk_rate = chunk as f64 * 1000.0 / timediff as f64;
    file.rate = (chunk_rate + 2.0 * file.rate) / 3.0;
}

fn ensure_download_file<'a>(state: &'a mut DownloadState, filename: &str) -> &'a mut DownloadFile {
    match state.files.entry(filename.to_string()) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(e) => {
            state.order.push(filename.to_string());
            e.insert(DownloadFile {
                downloaded: 0,
                total: 0,
                completed: false,
                rate: 0.0,
                sync_time: None,
                sync_done: 0,
            })
        }
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
    use crate::events::{DownloadResult, InstallEvent, PackageOp, ProgressPhase};
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
        RepoState::default()
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
        assert_eq!(state.download.total, 3);
        assert_eq!(state.download.done, 0);
        assert_eq!(state.download.bytes_total, 5000);
        assert_eq!(state.download.bytes_done, 0);
        assert!(state.download.files.is_empty());
    }

    #[test]
    fn apply_repo_retrieving_packages_clears_existing_map() {
        let mut state = fresh_repo_state();
        state.download.files.insert(
            "stale".to_string(),
            DownloadFile {
                downloaded: 999,
                total: 0,
                completed: false,
                rate: 0.0,
                sync_time: None,
                sync_done: 0,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 1,
                total_bytes: 10,
            },
        );
        assert!(state.download.files.is_empty());
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
        let f = state.download.files.get("a").expect("file a present");
        assert_eq!(f.downloaded, 100);
        assert_eq!(f.total, 200);
        assert!(!f.completed);
        assert_eq!(f.rate, 0.0);
        assert!(f.sync_time.is_some());
        assert_eq!(f.sync_done, 100);
        assert_eq!(state.download.bytes_done, 100);
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 150,
                total: 200,
            },
        );
        let f = state.download.files.get("a").expect("file a present");
        assert_eq!(f.downloaded, 150);
        assert_eq!(f.total, 200);
        assert!(!f.completed);
        assert_eq!(state.download.bytes_done, 150);
    }

    #[test]
    fn apply_repo_download_retry_full_resets_entry() {
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
        assert_eq!(
            state.download.files.get("a"),
            Some(&DownloadFile {
                downloaded: 0,
                total: 200,
                completed: false,
                rate: 0.0,
                sync_time: None,
                sync_done: 0,
            })
        );
        assert_eq!(state.download.bytes_done, 0);
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
        let f = state.download.files.get("a").expect("file a present");
        assert_eq!(f.downloaded, 100);
        assert_eq!(f.total, 200);
        assert!(!f.completed);
        assert_eq!(state.download.bytes_done, 100);
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
        assert_eq!(state.download.done, 1);
        assert_eq!(
            state.download.files.get("a"),
            Some(&DownloadFile {
                downloaded: 200,
                total: 200,
                completed: true,
                rate: 0.0,
                sync_time: None,
                sync_done: 0,
            })
        );
    }

    #[test]
    fn apply_repo_unrelated_event_is_noop() {
        let mut state = fresh_repo_state();
        apply_repo_counters(&mut state, &InstallEvent::ResolvingDependencies);
        assert_eq!(state.download.total, 0);
        assert_eq!(state.download.done, 0);
        assert_eq!(state.download.bytes_total, 0);
        assert_eq!(state.download.bytes_done, 0);
        assert!(state.download.files.is_empty());
    }

    #[test]
    fn apply_repo_download_order_preserves_insertion_order() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "a".to_string(),
                optional: false,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "b".to_string(),
                optional: false,
            },
        );
        assert_eq!(state.download.order, vec!["a".to_string(), "b".to_string()]);

        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 1,
                total_bytes: 0,
            },
        );
        assert!(state.download.order.is_empty());

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "a".to_string(),
                optional: false,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "b".to_string(),
                optional: false,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "a".to_string(),
                optional: false,
            },
        );
        assert_eq!(state.download.order, vec!["a".to_string(), "b".to_string()]);

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "a".to_string(),
                downloaded: 10,
                total: 100,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadRetry {
                filename: "a".to_string(),
                resume: false,
            },
        );
        assert_eq!(state.download.order, vec!["a".to_string(), "b".to_string()]);
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
            next_sysupgrade_step(
                SysupgradePage::PkgbuildReview,
                Direction::Forward,
                true,
                true
            ),
            SysupgradePage::Confirm
        );
    }

    #[test]
    fn next_sysupgrade_step_pkgbuild_review_backward_returns_resolve_when_present() {
        assert_eq!(
            next_sysupgrade_step(
                SysupgradePage::PkgbuildReview,
                Direction::Backward,
                true,
                true
            ),
            SysupgradePage::Resolve
        );
    }

    #[test]
    fn next_sysupgrade_step_pkgbuild_review_backward_skips_to_updates_without_resolve() {
        assert_eq!(
            next_sysupgrade_step(
                SysupgradePage::PkgbuildReview,
                Direction::Backward,
                false,
                true
            ),
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
