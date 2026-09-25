use std::collections::HashMap;
use std::time::Instant;

use crate::dispatch::exec::ChildOutcome;
use crate::download::TransferState;
use crate::events::{InstallEvent, LogLevel, PackageOp, ProgressPhase, TransactionSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Install,
    Remove,
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoStage {
    Resolve,
    Validate,
    Download,
    Install,
    Finalize,
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
pub struct InstallState {
    pub order: Vec<String>,
    pub packages: HashMap<String, InstallPackage>,
}

#[derive(Debug, Clone, Default)]
pub struct FinalizeState {
    pub lines: Vec<String>,
    pub alerts: Vec<(LogLevel, String)>,
}

impl FinalizeState {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.alerts.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct RepoState {
    pub manifest: Option<TransactionSummary>,
    pub resolve: ResolveState,
    pub validate: ValidateState,
    pub download: TransferState,
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
        usize::from(self.resolve.started)
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
        Progress { phase, .. } if !is_install_phase(phase) => Some(Validate),
        RetrievingPackages { .. }
        | DownloadInit { .. }
        | DownloadProgress { .. }
        | DownloadRetry { .. }
        | DownloadCompleted { .. }
        | RetrieveStart
        | RetrieveDone
        | RetrieveFailed
        | PkgRetrieveDone { .. }
        | PkgRetrieveFailed { .. } => Some(Download),
        Progress { .. } | PackageOperation { .. } | PackageOperationEnd { .. } => Some(Install),
        HookStart { .. }
        | HookRun { .. }
        | ScriptletInfo { .. }
        | TransactionDone
        | HookDone { .. }
        | HookRunDone
        | OptDepRemoval { .. }
        | DatabaseMissing { .. }
        | PacnewCreated { .. }
        | PacsaveCreated { .. } => Some(Finalize),
        ResolveDepsDone | CheckDepsDone | InterConflictsDone | FileConflictsDone
        | IntegrityDone | LoadDone | DiskSpaceDone | KeyringDone | KeyDownloadStart
        | KeyDownloadDone | SyncDatabases | StartSysupgrade => None,
        _ => None,
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
        InstallEvent::PackageOperationEnd { package, .. } => {
            if let Some(entry) = state.packages.get_mut(package) {
                entry.percent = 100.0;
                entry.completed = true;
            }
        }
        _ => {}
    }
}

fn apply_finalize(state: &mut FinalizeState, ev: &InstallEvent) {
    match ev {
        InstallEvent::OptDepRemoval { package, optdep } => {
            state
                .lines
                .push(format!("{package} optionally requires {optdep}"));
        }
        InstallEvent::DatabaseMissing { dbname } => {
            state.alerts.push((
                LogLevel::Warning,
                format!("database file for '{dbname}' does not exist (use '-Sy' to download)"),
            ));
        }
        InstallEvent::PacnewCreated { file, .. } => {
            state
                .alerts
                .push((LogLevel::Warning, crate::utils::pacnew_warning(file)));
        }
        InstallEvent::PacsaveCreated { file } => {
            state
                .alerts
                .push((LogLevel::Warning, crate::utils::pacsave_warning(file)));
        }
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
        InstallEvent::Log { level, message } => {
            let trimmed = message.trim_end();
            if !trimmed.is_empty() && matches!(level, LogLevel::Warning | LogLevel::Error) {
                state.alerts.push((*level, trimmed.to_string()));
            }
        }
        _ => {}
    }
}

pub fn apply_repo_counters(state: &mut RepoState, ev: &InstallEvent, now: Instant) {
    match ev {
        InstallEvent::ResolvingDependencies | InstallEvent::ResolvingAurDependencies { .. } => {
            state.resolve.started = true;
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
        InstallEvent::TransactionSummary(summary) => {
            state.resolve.started = true;
            state.resolve.checking = true;
            state.manifest = Some(summary.clone());
        }
        InstallEvent::Log { .. } => apply_finalize(&mut state.finalize, ev),
        _ => {}
    }
    match event_stage(ev) {
        Some(RepoStage::Validate) => track_validate_check(&mut state.validate, ev),
        Some(RepoStage::Install) => apply_install(&mut state.install, ev),
        Some(RepoStage::Download) => state.download.apply(ev, now),
        Some(RepoStage::Finalize) => apply_finalize(&mut state.finalize, ev),
        Some(RepoStage::Resolve) | None => {}
    }
}

pub fn ordered_stages(kind: InstallKind) -> &'static [RepoStage] {
    use InstallKind as Kind;
    use RepoStage::*;
    match kind {
        Kind::Remove => &[Resolve, Validate, Install, Finalize],
        Kind::Install | Kind::Upgrade => &[Resolve, Validate, Download, Install, Finalize],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AurStage {
    Resolve,
    Deps,
    Build,
    Install,
    Finalize,
}

pub fn ordered_aur_stages() -> &'static [AurStage] {
    use AurStage::*;
    &[Resolve, Deps, Build, Install, Finalize]
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

impl BuildStatus {
    fn in_flight(self) -> bool {
        matches!(self, Self::Fetching | Self::Building)
    }
}

pub const BUILD_TAIL_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub struct BuildPackage {
    pub status: BuildStatus,
    pub tail: Vec<String>,
    pub started: Instant,
    pub elapsed: Option<std::time::Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AurPhase {
    #[default]
    Deps,
    Artifacts,
}

#[derive(Debug, Clone, Default)]
pub struct AurState {
    pub resolve_started: bool,
    pub deps: HashMap<String, ResolvedDep>,
    pub dep_order: Vec<String>,
    pub resolve_complete: bool,
    pub phase: AurPhase,
    pub repo_deps: RepoState,
    pub repo_dep_count: usize,
    pub builds: HashMap<String, BuildPackage>,
    pub build_order: Vec<String>,
    pub build_ended: Option<Instant>,
    pub install: InstallState,
    pub download: TransferState,
    pub finalize: FinalizeState,
    pub last_aur_stage: Option<AurStage>,
}

impl AurState {
    pub fn building(&self) -> bool {
        self.builds.values().any(|entry| entry.status.in_flight())
    }
}

fn event_stage_aur(ev: &InstallEvent) -> Option<AurStage> {
    use AurStage::*;
    use InstallEvent::*;
    match ev {
        ResolvingAurDependencies { .. } | AurDepResolved { .. } | ResolutionComplete { .. } => {
            Some(Resolve)
        }
        CloningRepo { .. } | BuildStarted { .. } | BuildOutput { .. } | BuildCompleted { .. } => {
            Some(Build)
        }
        HookStart { .. } | HookRun { .. } | ScriptletInfo { .. } | TransactionDone => {
            Some(Finalize)
        }
        _ if event_stage(ev).is_some() || matches!(ev, TransactionSummary(_)) => Some(Install),
        _ => None,
    }
}

pub fn apply_aur_counters(state: &mut AurState, ev: &InstallEvent, now: Instant) {
    use InstallEvent::*;
    if matches!(
        ev,
        BuildStarted { .. } | BuildOutput { .. } | BuildCompleted { .. }
    ) {
        state.phase = AurPhase::Artifacts;
    }
    let stage = event_stage_aur(ev);
    let deps_event = state.phase == AurPhase::Deps
        && !matches!(ev, TransactionSummary(_))
        && (event_stage(ev).is_some() || matches!(ev, Log { .. }));
    if deps_event {
        apply_repo_counters(&mut state.repo_deps, ev, now);
        state.last_aur_stage = Some(AurStage::Deps);
    } else {
        state.last_aur_stage = stage.or(state.last_aur_stage);
    }
    match ev {
        ResolvingAurDependencies { .. } => state.resolve_started = true,
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
        ResolutionComplete { repo_deps, .. } => {
            state.resolve_complete = true;
            state.repo_dep_count = *repo_deps;
        }
        CloningRepo { package } => {
            if !state.builds.contains_key(package) {
                state.build_order.push(package.clone());
                state.builds.insert(
                    package.clone(),
                    BuildPackage {
                        status: BuildStatus::Fetching,
                        tail: Vec::new(),
                        started: now,
                        elapsed: None,
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
            if let Some(entry) = state.builds.get_mut(package)
                && let Some(segment) = line.rsplit('\r').next()
                && !crate::color::ansi_strip(segment).trim().is_empty()
            {
                entry.tail.push(segment.to_string());
                if entry.tail.len() > BUILD_TAIL_LIMIT {
                    entry.tail.remove(0);
                }
            }
        }
        BuildCompleted { package, .. } => {
            if let Some(entry) = state.builds.get_mut(package) {
                entry.status = BuildStatus::Done;
                entry.elapsed = Some(now.saturating_duration_since(entry.started));
            }
        }
        Log { .. } if !deps_event => apply_finalize(&mut state.finalize, ev),
        _ => {}
    }
    match (deps_event, stage) {
        (true, _) | (_, None) => {}
        (false, Some(AurStage::Install)) => match event_stage(ev) {
            Some(RepoStage::Install) => apply_install(&mut state.install, ev),
            Some(RepoStage::Download) => state.download.apply(ev, now),
            _ => {}
        },
        (false, Some(AurStage::Finalize)) => apply_finalize(&mut state.finalize, ev),
        (false, _) => {}
    }
    if !state.build_order.is_empty()
        && state.build_ended.is_none()
        && state.builds.values().all(|entry| !entry.status.in_flight())
    {
        state.build_ended = Some(now);
    }
}

pub fn finish_aur(state: &mut AurState, outcome: &ChildOutcome, now: Instant) {
    if matches!(outcome, ChildOutcome::Success) {
        return;
    }
    let failed = state
        .build_order
        .iter()
        .find(|name| {
            state
                .builds
                .get(*name)
                .is_some_and(|e| e.status.in_flight())
        })
        .cloned();
    if let Some(name) = failed
        && let Some(entry) = state.builds.get_mut(&name)
    {
        entry.status = BuildStatus::Failed;
        entry.elapsed = Some(now.saturating_duration_since(entry.started));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::events::DownloadResult;

    fn apply_repo_event(state: &mut RepoState, ev: &InstallEvent) {
        apply_repo_counters(state, ev, Instant::now());
    }

    fn log(level: LogLevel, message: &str) -> InstallEvent {
        InstallEvent::Log {
            level,
            message: message.to_string(),
        }
    }

    #[test]
    fn pacnew_created_surfaces_as_finalize_warning_alert() {
        let mut state = RepoState::default();
        apply_repo_event(
            &mut state,
            &InstallEvent::PacnewCreated {
                from_noupgrade: false,
                file: "/etc/pacman.conf".to_string(),
            },
        );
        assert_eq!(
            state.finalize.alerts,
            vec![(
                LogLevel::Warning,
                "/etc/pacman.conf installed as /etc/pacman.conf.pacnew".to_string()
            )]
        );
    }

    fn aur_dep(package: &str) -> InstallEvent {
        InstallEvent::AurDepResolved {
            package: package.to_string(),
            repo: None,
            version: Some("1.0-1".to_string()),
        }
    }

    fn retrieving(num: usize, total_bytes: i64) -> InstallEvent {
        InstallEvent::RetrievingPackages { num, total_bytes }
    }

    fn init(name: &str) -> InstallEvent {
        InstallEvent::DownloadInit {
            filename: name.to_string(),
            optional: false,
        }
    }

    fn file_progress(name: &str, downloaded: i64, total: i64) -> InstallEvent {
        InstallEvent::DownloadProgress {
            filename: name.to_string(),
            downloaded,
            total,
        }
    }

    fn file_retry(name: &str, resume: bool) -> InstallEvent {
        InstallEvent::DownloadRetry {
            filename: name.to_string(),
            resume,
        }
    }

    fn file_done(name: &str, total: i64) -> InstallEvent {
        InstallEvent::DownloadCompleted {
            filename: name.to_string(),
            total,
            result: DownloadResult::Success,
        }
    }

    fn pkg_added(name: &str) -> InstallEvent {
        InstallEvent::PackageOperation {
            operation: PackageOp::Install,
            package: name.to_string(),
            new_version: Some("6.1".to_string()),
            old_version: None,
        }
    }

    fn pkg_progress(name: &str, percent: i32) -> InstallEvent {
        InstallEvent::Progress {
            phase: ProgressPhase::Add,
            package: name.to_string(),
            percent,
            current: 1,
            total: 1,
        }
    }

    fn hook(position: usize, total: usize, label: &str) -> InstallEvent {
        InstallEvent::HookRun {
            position,
            total,
            name: "hook".to_string(),
            desc: Some(label.to_string()),
        }
    }

    fn spawn_build(state: &mut AurState, name: &str, now: Instant) {
        apply_aur_counters(
            state,
            &InstallEvent::CloningRepo {
                package: name.to_string(),
            },
            now,
        );
    }

    fn build_line(state: &mut AurState, name: &str, line: &str, now: Instant) {
        apply_aur_counters(
            state,
            &InstallEvent::BuildOutput {
                package: name.to_string(),
                line: line.to_string(),
            },
            now,
        );
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

    fn build<'a>(state: &'a AurState, name: &str) -> &'a BuildPackage {
        state.builds.get(name).expect(name)
    }

    #[test]
    fn aur_dep_resolved_drives_repo_resolve_to_checking() {
        let mut state = RepoState::default();
        apply_repo_event(&mut state, &aur_dep("yay"));
        assert_eq!(state.resolve_step(), 2);
    }

    #[test]
    fn apply_repo_download_lifecycle_tracks_progress_retry_and_reset() {
        let mut state = RepoState::default();
        apply_repo_event(&mut state, &retrieving(2, 1000));
        assert_eq!(
            (state.download.total, state.download.bytes_total),
            (2, 1000)
        );
        assert_eq!(state.download.queued(), 2);

        for name in ["pkg-a", "pkg-b", "pkg-a"] {
            apply_repo_event(&mut state, &init(name));
        }
        assert_eq!(state.download.order, ["pkg-a", "pkg-b"]);

        apply_repo_event(&mut state, &file_progress("pkg-a", 400, 400));
        assert_eq!(state.download.bytes_done, 400);
        apply_repo_event(&mut state, &file_done("pkg-a", 400));
        assert_eq!(state.download.done, 1);

        apply_repo_event(&mut state, &file_progress("pkg-b", 300, 600));
        assert_eq!(state.download.bytes_done, 700);
        apply_repo_event(&mut state, &file_retry("pkg-b", true));
        assert_eq!(state.download.bytes_done, 700);
        apply_repo_event(&mut state, &file_retry("pkg-b", false));
        assert_eq!(state.download.bytes_done, 400);
        assert_eq!(state.download.files["pkg-b"].downloaded, 0);

        apply_repo_event(&mut state, &file_progress("pkg-b", 300, 600));
        assert_eq!(state.download.bytes_done, 700);
        apply_repo_event(&mut state, &file_done("pkg-b", 600));
        assert_eq!(state.download.done, 2);
        assert_eq!(state.download.bytes_done, 700);
        assert_eq!(state.download.queued(), 0);

        apply_repo_event(
            &mut state,
            &InstallEvent::PkgRetrieveDone {
                num: 2,
                total_bytes: 1000,
            },
        );
        assert_eq!(state.download.bytes_done, 1000);

        apply_repo_event(&mut state, &retrieving(1, 10));
        assert_eq!(state.download.total, 1);
        assert_eq!((state.download.done, state.download.bytes_done), (0, 0));
        assert!(state.download.files.is_empty());
        assert!(state.download.order.is_empty());
    }

    #[test]
    fn apply_repo_counters_drives_full_transaction() {
        let mut state = RepoState::default();
        assert_eq!(state.resolve_step(), 0);
        apply_repo_event(&mut state, &InstallEvent::ResolvingDependencies);
        assert_eq!(state.resolve_step(), 1);
        apply_repo_event(&mut state, &InstallEvent::CheckingConflicts);
        assert_eq!(state.resolve_step(), 2);
        apply_repo_event(&mut state, &InstallEvent::CheckingDiskSpace);
        assert_eq!(state.validate.count(), 2);

        apply_repo_event(&mut state, &pkg_added("pacman"));
        apply_repo_event(&mut state, &pkg_progress("pacman", 100));
        assert!(state.install.packages["pacman"].completed);

        apply_repo_event(&mut state, &hook(1, 1, "Updating font cache..."));
        assert_eq!(state.finalize.lines, ["(1/1) Updating font cache..."]);

        apply_repo_event(
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
    fn apply_repo_counters_collects_log_alerts() {
        let mut state = RepoState::default();
        apply_repo_event(&mut state, &log(LogLevel::Warning, "dep cycle\n"));
        apply_repo_event(&mut state, &log(LogLevel::Debug, "noise\n"));
        apply_repo_event(&mut state, &log(LogLevel::Error, "  \n"));
        apply_repo_event(&mut state, &log(LogLevel::Error, "unknown key\n"));
        assert_eq!(
            state.finalize.alerts,
            [
                (LogLevel::Warning, "dep cycle".to_string()),
                (LogLevel::Error, "unknown key".to_string())
            ]
        );
        assert!(state.finalize.lines.is_empty());
    }

    #[test]
    fn build_tail_caps_at_limit() {
        let now = Instant::now();
        let mut state = AurState::default();
        spawn_build(&mut state, "yay", now);
        for i in 1..=205 {
            build_line(&mut state, "yay", &format!("line {i}"), now);
        }
        assert_eq!(build(&state, "yay").tail.len(), BUILD_TAIL_LIMIT);
        assert_eq!(build(&state, "yay").tail[0], "line 6");
        build_line(&mut state, "yay", "1%\r2%\r3%\r 100%", now);
        assert_eq!(
            build(&state, "yay").tail.last().map(String::as_str),
            Some(" 100%")
        );
    }

    #[test]
    fn build_tail_stores_raw_ansi_verbatim() {
        let mut state = AurState::default();
        spawn_build(&mut state, "yay", Instant::now());
        let raw = "\x1b[1m==>\x1b[0m pkg";
        build_line(&mut state, "yay", raw, Instant::now());
        assert_eq!(build(&state, "yay").tail, [raw]);
    }

    #[test]
    fn finish_aur_success_leaves_state_untouched() {
        let now = Instant::now();
        let mut state = AurState::default();
        spawn_build(&mut state, "pkg-a", now);
        finish_aur(&mut state, &ChildOutcome::Success, now);
        assert_eq!(build(&state, "pkg-a").status, BuildStatus::Fetching);
        assert!(build(&state, "pkg-a").elapsed.is_none());
        assert_eq!(state.build_ended, None);
    }

    #[test]
    fn finish_aur_flips_first_in_flight_card() {
        let now = Instant::now();
        let mut state = AurState::default();
        spawn_build(&mut state, "done-pkg", now);
        spawn_build(&mut state, "live-pkg", now);
        state.builds.get_mut("done-pkg").unwrap().status = BuildStatus::Done;
        finish_aur(
            &mut state,
            &ChildOutcome::Failed("makepkg failed".to_string()),
            now,
        );
        assert_eq!(build(&state, "done-pkg").status, BuildStatus::Done);
        assert_eq!(build(&state, "live-pkg").status, BuildStatus::Failed);
        assert!(build(&state, "live-pkg").elapsed.is_some());
    }

    #[test]
    fn deps_phase_routes_generic_vocab_to_repo_deps_bucket() {
        let now = Instant::now();
        let mut state = AurState::default();
        assert_eq!(state.phase, AurPhase::Deps);
        apply_aur_counters(&mut state, &retrieving(1, 100), now);
        apply_aur_counters(&mut state, &init("dep1"), now);
        apply_aur_counters(&mut state, &file_done("dep1", 100), now);
        apply_aur_counters(&mut state, &pkg_added("dep1"), now);
        apply_aur_counters(&mut state, &pkg_progress("dep1", 100), now);
        apply_aur_counters(&mut state, &hook(1, 1, "Arming..."), now);
        apply_aur_counters(
            &mut state,
            &InstallEvent::ScriptletInfo {
                line: "scriptlet".to_string(),
            },
            now,
        );
        apply_aur_counters(&mut state, &log(LogLevel::Warning, "dep warn\n"), now);
        assert_eq!(state.phase, AurPhase::Deps);
        assert_eq!(state.last_aur_stage, Some(AurStage::Deps));
        assert_eq!(state.repo_deps.install.order, ["dep1"]);
        assert!(state.repo_deps.install.packages["dep1"].completed);
        assert_eq!(state.repo_deps.download.done, 1);
        assert_eq!(state.repo_deps.finalize.lines.len(), 2);
        assert_eq!(state.repo_deps.finalize.alerts.len(), 1);
        assert!(state.install.order.is_empty());
        assert!(state.finalize.is_empty());
        assert_eq!(state.download.total, 0);
    }

    #[test]
    fn cloning_repo_does_not_flip_phase() {
        let now = Instant::now();
        let mut state = AurState::default();
        apply_aur_counters(
            &mut state,
            &InstallEvent::CloningRepo {
                package: "pkg-a".to_string(),
            },
            now,
        );
        assert_eq!(state.phase, AurPhase::Deps);
        assert_eq!(build(&state, "pkg-a").status, BuildStatus::Fetching);
        apply_aur_counters(&mut state, &pkg_added("dep1"), now);
        assert_eq!(state.repo_deps.install.order, ["dep1"]);
        assert!(state.install.order.is_empty());
    }

    #[test]
    fn build_started_flips_phase_monotonically() {
        let now = Instant::now();
        let mut state = AurState::default();
        apply_aur_counters(
            &mut state,
            &InstallEvent::CloningRepo {
                package: "pkg-a".to_string(),
            },
            now,
        );
        apply_aur_counters(&mut state, &build_started("pkg-a"), now);
        assert_eq!(state.phase, AurPhase::Artifacts);
        apply_aur_counters(&mut state, &pkg_added("pkg-a"), now);
        assert_eq!(state.install.order, ["pkg-a"]);
        assert!(state.repo_deps.install.order.is_empty());
        assert_eq!(state.last_aur_stage, Some(AurStage::Install));
    }

    #[test]
    fn build_output_and_completed_also_flip_phase() {
        let now = Instant::now();
        let mut state = AurState::default();
        apply_aur_counters(
            &mut state,
            &InstallEvent::BuildOutput {
                package: "pkg-a".to_string(),
                line: "output".to_string(),
            },
            now,
        );
        assert_eq!(state.phase, AurPhase::Artifacts);
        let mut late = AurState::default();
        apply_aur_counters(&mut late, &build_completed("pkg-a"), now);
        assert_eq!(late.phase, AurPhase::Artifacts);
    }

    #[test]
    fn transaction_summary_dropped_in_both_phases() {
        let now = Instant::now();
        let mut state = AurState::default();
        let summary = InstallEvent::TransactionSummary(TransactionSummary {
            packages: Vec::new(),
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        });
        apply_aur_counters(&mut state, &summary, now);
        assert!(state.repo_deps.manifest.is_none());
        apply_aur_counters(&mut state, &build_started("pkg-a"), now);
        apply_aur_counters(&mut state, &summary, now);
        assert!(state.repo_deps.manifest.is_none());
        assert!(state.install.order.is_empty());
    }

    #[test]
    fn resolution_complete_records_repo_dep_count() {
        let now = Instant::now();
        let mut state = AurState::default();
        apply_aur_counters(
            &mut state,
            &InstallEvent::ResolutionComplete {
                aur_packages: 1,
                repo_deps: 3,
            },
            now,
        );
        assert!(state.resolve_complete);
        assert_eq!(state.repo_dep_count, 3);
    }

    #[test]
    fn build_clocks_freeze_at_terminal() {
        let base = Instant::now();
        let mut state = AurState::default();
        for package in ["pkg-a", "pkg-b"] {
            spawn_build(&mut state, package, base);
        }
        apply_aur_counters(
            &mut state,
            &build_started("pkg-a"),
            base + Duration::from_secs(10),
        );
        apply_aur_counters(
            &mut state,
            &build_completed("pkg-a"),
            base + Duration::from_secs(30),
        );
        assert_eq!(
            build(&state, "pkg-a").elapsed,
            Some(Duration::from_secs(30))
        );
        assert_eq!(state.build_ended, None);
        let last = base + Duration::from_secs(90);
        apply_aur_counters(&mut state, &build_completed("pkg-b"), last);
        assert_eq!(
            build(&state, "pkg-b").elapsed,
            Some(Duration::from_secs(90))
        );
        assert_eq!(state.build_ended, Some(last));
    }
}
