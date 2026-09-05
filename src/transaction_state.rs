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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AurStage {
    Resolve,
    Build,
    Install,
    Finalize,
}

pub fn ordered_aur_stages() -> Vec<AurStage> {
    use AurStage::*;
    vec![Resolve, Build, Install, Finalize]
}

#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub repository: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Fetching,
    Building,
    Done,
    Failed,
}

pub const BUILD_TAIL_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub struct BuildPackage {
    pub status: BuildStatus,
    pub tail: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AurState {
    pub resolve_started: bool,
    pub deps: HashMap<String, ResolvedDep>,
    pub dep_order: Vec<String>,
    pub resolve_complete: bool,
    pub builds: HashMap<String, BuildPackage>,
    pub build_order: Vec<String>,
    pub install: InstallState,
    pub download: DownloadState,
    pub finalize: FinalizeState,
    pub last_aur_stage: Option<AurStage>,
}

pub fn event_stage_aur(ev: &InstallEvent) -> Option<AurStage> {
    use AurStage::*;
    use InstallEvent::*;
    match ev {
        ResolvingAurDependencies { .. } | AurDepResolved { .. } | ResolutionComplete { .. } => {
            Some(Resolve)
        }
        CloningRepo { .. } | BuildStarted { .. } | BuildOutput { .. } | BuildCompleted { .. } => {
            Some(Build)
        }
        ResolvingDependencies
        | CheckingConflicts
        | CheckingDependencies
        | CheckingFileConflicts
        | CheckingIntegrity
        | CheckingDiskSpace
        | KeyringStart
        | LoadingPackages
        | TransactionSummary(_)
        | RetrievingPackages { .. }
        | DownloadInit { .. }
        | DownloadProgress { .. }
        | DownloadRetry { .. }
        | DownloadCompleted { .. }
        | PackageOperation { .. }
        | Progress { .. } => Some(Install),
        HookRun { .. } | ScriptletInfo { .. } | TransactionDone => Some(Finalize),
        _ => None,
    }
}

pub fn apply_aur_counters(state: &mut AurState, ev: &InstallEvent) {
    use InstallEvent::*;
    if let Some(stage) = event_stage_aur(ev) {
        state.last_aur_stage = Some(stage);
    }
    match ev {
        ResolvingAurDependencies { .. } => {
            state.resolve_started = true;
        }
        AurDepResolved {
            package,
            repo,
            version,
        } => {
            if !state.deps.contains_key(package) {
                state.dep_order.push(package.clone());
            }
            state.deps.insert(
                package.clone(),
                ResolvedDep {
                    repository: repo.clone(),
                    version: version.clone(),
                },
            );
        }
        ResolutionComplete { .. } => {
            state.resolve_complete = true;
        }
        CloningRepo { package } => {
            if !state.builds.contains_key(package) {
                state.build_order.push(package.clone());
                state.builds.insert(
                    package.clone(),
                    BuildPackage {
                        status: BuildStatus::Fetching,
                        tail: Vec::new(),
                    },
                );
            }
        }
        BuildStarted { package } => {
            if let Some(entry) = state.builds.get_mut(package) {
                entry.status = BuildStatus::Building;
            }
        }
        BuildOutput { package, line } => {
            if let Some(entry) = state.builds.get_mut(package) {
                let segment = line.rsplit('\r').next().unwrap_or("");
                let stripped = crate::color::ansi_strip(segment);
                if !stripped.trim().is_empty() {
                    entry.tail.push(stripped);
                    if entry.tail.len() > BUILD_TAIL_LIMIT {
                        entry.tail.remove(0);
                    }
                }
            }
        }
        BuildCompleted { package, .. } => {
            if let Some(entry) = state.builds.get_mut(package) {
                entry.status = BuildStatus::Done;
            }
        }
        PackageOperation { .. } => apply_install(&mut state.install, ev),
        Progress { phase, .. } if is_install_phase(phase) => apply_install(&mut state.install, ev),
        RetrievingPackages { .. }
        | DownloadInit { .. }
        | DownloadProgress { .. }
        | DownloadRetry { .. }
        | DownloadCompleted { .. } => apply_download(&mut state.download, ev),
        HookRun { .. } | ScriptletInfo { .. } => apply_finalize(&mut state.finalize, ev),
        _ => {}
    }
}

pub fn finish_aur(state: &mut AurState, outcome: &crate::install::ChildOutcome) {
    use crate::install::ChildOutcome;
    if matches!(outcome, ChildOutcome::Success) {
        return;
    }
    let failed = state
        .build_order
        .iter()
        .find(|name| {
            state.builds.get(*name).is_some_and(|entry| {
                matches!(entry.status, BuildStatus::Fetching | BuildStatus::Building)
            })
        })
        .cloned();
    if let Some(name) = failed
        && let Some(entry) = state.builds.get_mut(&name)
    {
        entry.status = BuildStatus::Failed;
    }
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
    use crate::events::{
        DownloadResult, InstallEvent, PackageOp, ProgressPhase, TransactionSummary,
    };
    use crate::install::ChildOutcome;

    fn fresh_repo_state() -> RepoState {
        RepoState::default()
    }

    #[test]
    fn apply_repo_download_lifecycle_tracks_progress_retry_and_reset() {
        let mut state = fresh_repo_state();
        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 2,
                total_bytes: 1000,
            },
        );
        assert_eq!(state.download.total, 2);
        assert_eq!(state.download.bytes_total, 1000);
        assert_eq!(state.download.queued(), 2);

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "pkg-a".to_string(),
                optional: false,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "pkg-b".to_string(),
                optional: false,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadInit {
                filename: "pkg-a".to_string(),
                optional: false,
            },
        );
        assert_eq!(
            state.download.order,
            vec!["pkg-a".to_string(), "pkg-b".to_string()]
        );

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "pkg-a".to_string(),
                downloaded: 400,
                total: 400,
            },
        );
        assert_eq!(state.download.bytes_done, 400);
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadCompleted {
                filename: "pkg-a".to_string(),
                total: 400,
                result: DownloadResult::Success,
            },
        );
        assert_eq!(state.download.done, 1);

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadProgress {
                filename: "pkg-b".to_string(),
                downloaded: 300,
                total: 600,
            },
        );
        assert_eq!(state.download.bytes_done, 700);
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadRetry {
                filename: "pkg-b".to_string(),
                resume: true,
            },
        );
        assert_eq!(state.download.bytes_done, 700);
        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadRetry {
                filename: "pkg-b".to_string(),
                resume: false,
            },
        );
        assert_eq!(state.download.bytes_done, 400);
        let pkg_b = state.download.files.get("pkg-b").expect("pkg-b present");
        assert_eq!(pkg_b.downloaded, 0);

        apply_repo_counters(
            &mut state,
            &InstallEvent::DownloadCompleted {
                filename: "pkg-b".to_string(),
                total: 600,
                result: DownloadResult::Success,
            },
        );
        assert_eq!(state.download.done, 2);
        assert_eq!(state.download.bytes_done, 1000);
        assert_eq!(state.download.queued(), 0);

        apply_repo_counters(
            &mut state,
            &InstallEvent::RetrievingPackages {
                num: 1,
                total_bytes: 10,
            },
        );
        assert_eq!(state.download.total, 1);
        assert_eq!(state.download.done, 0);
        assert_eq!(state.download.bytes_done, 0);
        assert!(state.download.files.is_empty());
        assert!(state.download.order.is_empty());
    }

    #[test]
    fn apply_repo_counters_drives_full_transaction() {
        let mut state = fresh_repo_state();
        assert_eq!(state.resolve_step(), 0);
        apply_repo_counters(&mut state, &InstallEvent::ResolvingDependencies);
        assert_eq!(state.resolve_step(), 1);
        apply_repo_counters(&mut state, &InstallEvent::CheckingConflicts);
        assert_eq!(state.resolve_step(), 2);
        apply_repo_counters(&mut state, &InstallEvent::CheckingDiskSpace);
        assert_eq!(state.validate.count(), 2);

        apply_repo_counters(
            &mut state,
            &InstallEvent::PackageOperation {
                operation: PackageOp::Install,
                package: "pacman".to_string(),
                new_version: Some("6.1".to_string()),
                old_version: None,
            },
        );
        apply_repo_counters(
            &mut state,
            &InstallEvent::Progress {
                phase: ProgressPhase::Add,
                package: "pacman".to_string(),
                percent: 100,
                current: 1,
                total: 1,
            },
        );
        let pkg = state
            .install
            .packages
            .get("pacman")
            .expect("pacman present");
        assert!(pkg.completed);

        apply_repo_counters(
            &mut state,
            &InstallEvent::HookRun {
                position: 1,
                total: 1,
                name: "hook".to_string(),
                desc: Some("Updating font cache...".to_string()),
            },
        );
        assert_eq!(
            state.finalize.lines,
            vec!["(1/1) Updating font cache...".to_string()]
        );

        apply_repo_counters(
            &mut state,
            &InstallEvent::TransactionSummary(TransactionSummary {
                packages: Vec::new(),
                total_download_size: 0,
                total_installed_size: 0,
                total_removed_size: 0,
            }),
        );
        assert_eq!(state.resolve_step(), 3);
        assert!(state.manifest.is_some());
    }

    #[test]
    fn classify_outcome_routes_child_results() {
        let aur_targets = vec!["aur-pkg".to_string()];
        let cases = [
            (
                ChildOutcome::Success,
                Some(SysupgradePhase::Repo),
                aur_targets.clone(),
                NextInstallState::ContinueAur {
                    targets: aur_targets.clone(),
                },
            ),
            (
                ChildOutcome::Success,
                Some(SysupgradePhase::Repo),
                Vec::new(),
                NextInstallState::Completed,
            ),
            (
                ChildOutcome::Success,
                None,
                Vec::new(),
                NextInstallState::Completed,
            ),
            (
                ChildOutcome::Dismissed,
                None,
                Vec::new(),
                NextInstallState::Cancelled,
            ),
            (
                ChildOutcome::NotFound,
                None,
                Vec::new(),
                NextInstallState::Failed {
                    message: "install child not found".to_string(),
                },
            ),
            (
                ChildOutcome::Failed("err".to_string()),
                None,
                Vec::new(),
                NextInstallState::Failed {
                    message: "err".to_string(),
                },
            ),
        ];
        for (outcome, phase, targets, expected) in cases {
            assert_eq!(
                classify_outcome(&outcome, phase, &targets),
                expected,
                "outcome {outcome:?} phase {phase:?} targets {targets:?}"
            );
        }
    }

    #[test]
    fn next_sysupgrade_step_routes_navigation() {
        use Direction::*;
        use SysupgradePage::*;
        let cases = [
            (Updates, Forward, false, false, Confirm),
            (Updates, Forward, false, true, PkgbuildReview),
            (Updates, Forward, true, true, Resolve),
            (Resolve, Forward, true, true, PkgbuildReview),
            (Resolve, Forward, true, false, Confirm),
            (Resolve, Backward, true, true, Updates),
            (PkgbuildReview, Forward, true, true, Confirm),
            (PkgbuildReview, Backward, true, true, Resolve),
            (PkgbuildReview, Backward, false, true, Updates),
            (Confirm, Backward, true, true, PkgbuildReview),
            (Confirm, Backward, true, false, Resolve),
            (Confirm, Backward, false, false, Updates),
            (Confirm, Forward, true, true, Confirm),
            (Updates, Backward, true, true, Updates),
        ];
        for (from, dir, has_resolve, has_diffs, expected) in cases {
            assert_eq!(
                next_sysupgrade_step(from, dir, has_resolve, has_diffs),
                expected,
                "from {from:?} dir {dir:?} has_resolve {has_resolve} has_diffs {has_diffs}"
            );
        }
    }

    #[test]
    fn build_tail_caps_at_limit() {
        let mut state = AurState::default();
        apply_aur_counters(
            &mut state,
            &InstallEvent::CloningRepo {
                package: "yay".to_string(),
            },
        );
        for i in 1..=205 {
            apply_aur_counters(
                &mut state,
                &InstallEvent::BuildOutput {
                    package: "yay".to_string(),
                    line: format!("line {i}"),
                },
            );
        }
        let entry = state.builds.get("yay").expect("yay present");
        assert_eq!(entry.tail.len(), BUILD_TAIL_LIMIT);
        assert_eq!(entry.tail[0], "line 6");
        apply_aur_counters(
            &mut state,
            &InstallEvent::BuildOutput {
                package: "yay".to_string(),
                line: "1%\r2%\r3%\r 100%".to_string(),
            },
        );
        let entry = state.builds.get("yay").expect("yay present");
        assert_eq!(entry.tail.last().expect("tail nonempty"), " 100%");
    }

    #[test]
    fn finish_aur_flips_first_in_flight_card() {
        let mut state = AurState::default();
        for name in ["done-pkg", "live-pkg"] {
            state.build_order.push(name.to_string());
            state.builds.insert(
                name.to_string(),
                BuildPackage {
                    status: BuildStatus::Fetching,
                    tail: Vec::new(),
                },
            );
        }
        state
            .builds
            .get_mut("done-pkg")
            .expect("done-pkg present")
            .status = BuildStatus::Done;
        state
            .builds
            .get_mut("live-pkg")
            .expect("live-pkg present")
            .status = BuildStatus::Building;
        finish_aur(
            &mut state,
            &ChildOutcome::Failed("makepkg failed".to_string()),
        );
        assert_eq!(
            state
                .builds
                .get("done-pkg")
                .expect("done-pkg present")
                .status,
            BuildStatus::Done
        );
        assert_eq!(
            state
                .builds
                .get("live-pkg")
                .expect("live-pkg present")
                .status,
            BuildStatus::Failed
        );
    }
}
