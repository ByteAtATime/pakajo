use crate::{
    aur::AurClient,
    events::{InstallEvent, LogLevel},
    install::{ChildOutcome, InstallProgress, StreamItem},
    local_index::LocalIndex,
    package::{Package, PackageSource, installed_names, is_installed},
    pacman::{find_pkg, init_alpm},
    pkgbuild::{PkgbuildDiff, prepare_pkgbuild_diffs},
    question::{QuestionSet, encode_approvals},
    search::engine::SearchEngine,
    search::{self, SearchResult},
};
use alpm::Alpm;
use anyhow::Context as _;
use futures::StreamExt as _;
use gpui::*;
use std::{env::current_exe, sync::Arc, time::Duration};

const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);
const LOCK_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum DetailData {
    None,
    Loading,
    Ready { pkg: Package, installed: bool },
    Error(String),
    Group { name: String, members: Vec<GroupMember> },
}

#[derive(Clone, Debug)]
pub(crate) struct GroupMember {
    pub name: String,
    pub description: Option<String>,
    pub installed: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SearchState {
    Idle,
    Searching,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UpdatesState {
    Idle,
    Loading,
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallKind {
    Install,
    Remove,
    Upgrade,
}

pub(crate) enum SessionEvent {
    DetailUpdated,
    SearchUpdated,
    InstallProgressChanged(InstallProgress),
    InstallLog(InstallEvent),
    InstallLogsOpened {
        kind: InstallKind,
        source: PackageSource,
        name: String,
    },
    ReviewRequired {
        qs: QuestionSet,
        name: String,
    },
    PkgbuildReviewRequired {
        diffs: Vec<PkgbuildDiff>,
    },
    UpdatesAvailable(u32),
    SysupgradePreviewReady(Result<crate::dry_run::SysupgradePreview, String>),
}

impl EventEmitter<SessionEvent> for PakajoSession {}

struct PendingInstall {
    name: String,
    source: PackageSource,
    approvals_b64: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SysupgradePhase {
    Repo,
    Aur,
}

pub(crate) struct PakajoSession {
    pub(crate) alpm_handle: Alpm,
    pub(crate) aur_client: Arc<AurClient>,
    pub(crate) search_engine: Option<Arc<SearchEngine>>,
    pub(crate) local_index: Option<Arc<LocalIndex>>,
    pub(crate) installed_names: Arc<std::collections::HashSet<String>>,
    pub(crate) group_index: Arc<Vec<(String, String)>>,
    pub(crate) detail: DetailData,
    detail_seq: u64,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) selected_index: Option<usize>,
    search_seq: u64,
    pub(crate) search_state: SearchState,
    install_progress: InstallProgress,
    pending_install: Option<PendingInstall>,
    pub(crate) pending_count: u32,
    pub(crate) updates_state: UpdatesState,
    pub(crate) pending_updates: crate::updates::PendingUpdates,
    pub(crate) updates_aur_error: Option<String>,
    pub(crate) sysupgrade_preview_in_flight: bool,
    active_sysupgrade_phase: Option<SysupgradePhase>,
    sysupgrade_aur_targets: Vec<String>,
}

impl PakajoSession {
    pub(crate) fn new(alpm_handle: Alpm, aur_client: AurClient) -> Self {
        let search_engine = LocalIndex::db_path()
            .ok()
            .and_then(|p| SearchEngine::new(p).ok())
            .map(Arc::new);
        let local_index = LocalIndex::db_path()
            .ok()
            .and_then(|p| {
                LocalIndex::open(&p)
                    .map_err(|e| {
                        eprintln!("local index unavailable, falling back to live search: {e:#}")
                    })
                    .ok()
            })
            .map(Arc::new);
        let installed_names = Arc::new(installed_names(&alpm_handle));
        let group_index = Arc::new(crate::pacman::collect_group_index(&alpm_handle));
        let aur_client = Arc::new(aur_client);
        if let Some(index) = &local_index {
            begin_aur_sync_in_background(index.clone(), search_engine.clone());
        }
        Self {
            alpm_handle,
            aur_client,
            search_engine,
            local_index,
            installed_names,
            group_index,
            detail: DetailData::None,
            detail_seq: 0,
            results: Vec::new(),
            selected_index: None,
            search_seq: 0,
            search_state: SearchState::Idle,
            install_progress: InstallProgress::Idle,
            pending_install: None,
            pending_count: 0,
            updates_state: UpdatesState::Idle,
            pending_updates: crate::updates::PendingUpdates {
                repo: Vec::new(),
                aur: Vec::new(),
            },
            updates_aur_error: None,
            sysupgrade_preview_in_flight: false,
            active_sysupgrade_phase: None,
            sysupgrade_aur_targets: Vec::new(),
        }
    }

    pub(crate) fn load_detail(
        &mut self,
        name: String,
        source: PackageSource,
        cx: &mut Context<Self>,
    ) {
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let seq = self.detail_seq;
        self.detail = DetailData::Loading;
        cx.emit(SessionEvent::DetailUpdated);

        match source {
            PackageSource::Repo => {
                if let Some(pkg) = find_pkg(&self.alpm_handle, &name) {
                    self.set_detail(Package::from(pkg), cx);
                } else {
                    self.detail = DetailData::Error(format!("package not found: {name}"));
                    cx.emit(SessionEvent::DetailUpdated);
                }
            }
            PackageSource::Aur => {
                let aur = self.aur_client.clone();
                let local_index = self.local_index.clone();
                let name_for_info = name.clone();
                let name_for_error = name.clone();

                cx.spawn(async move |this, cx| {
                    let cached = if let Some(index) = local_index.clone() {
                        let name_for_cache = name_for_info.clone();
                        cx.background_executor()
                            .spawn(async move { index.detail(&name_for_cache) })
                            .await
                    } else {
                        Ok(None)
                    };

                    let had_cache = matches!(cached, Ok(Some(_)));
                    match cached {
                        Ok(Some(info)) => {
                            let _ = this.update(cx, |s, cx| {
                                if s.detail_seq == seq {
                                    s.set_detail(Package::from(info), cx);
                                }
                            });
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("[pakajo] detail cache read failed: {e:#}"),
                    }

                    if had_cache {
                        cx.background_executor().timer(DETAIL_DEBOUNCE).await;
                        let still_valid = this
                            .update(cx, |s, _cx| s.detail_seq == seq)
                            .unwrap_or(false);
                        if !still_valid {
                            return;
                        }
                    }

                    let index_for_cache = local_index.clone();
                    let info = cx
                        .background_executor()
                        .spawn(async move {
                            let info = aur.info(&name_for_info);
                            if let Ok(Some(ref a)) = info
                                && let Some(index) = index_for_cache.as_ref()
                            {
                                let _ = index.put_detail(a).map_err(|e| {
                                    eprintln!("[pakajo] detail put_detail failed: {e:#}")
                                });
                            }
                            info
                        })
                        .await;

                    let _ = this.update(cx, |s, cx| {
                        if s.detail_seq != seq {
                            return;
                        }
                        match info {
                            Ok(Some(a)) => s.set_detail(Package::from(a), cx),
                            Ok(None) if matches!(s.detail, DetailData::Loading) => {
                                s.detail = DetailData::Error(format!(
                                    "package not found: {name_for_error}"
                                ));
                                cx.emit(SessionEvent::DetailUpdated);
                            }
                            Ok(None) => {}
                            Err(e) if matches!(s.detail, DetailData::Loading) => {
                                s.detail = DetailData::Error(search::friendly_search_error(&e));
                                cx.emit(SessionEvent::DetailUpdated);
                            }
                            Err(_) => {}
                        }
                    });
                })
                .detach();
            }
            PackageSource::Group => {
                match crate::pacman::find_groups(&self.alpm_handle, &name)
                    .into_iter()
                    .next()
                {
                    Some((_, group)) => {
                        let members: Vec<GroupMember> = group
                            .packages()
                            .iter()
                            .map(|p| GroupMember {
                                name: p.name().to_string(),
                                description: p.desc().map(|d| d.to_string()),
                                installed: self.installed_names.contains(p.name()),
                            })
                            .collect();
                        self.detail = DetailData::Group { name, members };
                    }
                    None => {
                        self.detail = DetailData::Error(format!("group not found: {name}"));
                    }
                }
                cx.emit(SessionEvent::DetailUpdated);
            }
        }
    }

    fn set_detail(&mut self, pkg: Package, cx: &mut Context<Self>) {
        let installed = is_installed(&self.alpm_handle, &pkg.name);
        self.detail = DetailData::Ready { pkg, installed };
        cx.emit(SessionEvent::DetailUpdated);
    }

    pub(crate) fn refresh_detail_installed(&mut self, cx: &mut Context<Self>) {
        if let DetailData::Ready { pkg, .. } = &self.detail {
            let installed = is_installed(&self.alpm_handle, &pkg.name);
            let pkg = pkg.clone();
            self.detail = DetailData::Ready { pkg, installed };
            cx.emit(SessionEvent::DetailUpdated);
        }
    }

    pub(crate) fn set_progress(&mut self, progress: InstallProgress, cx: &mut Context<Self>) {
        self.install_progress = progress.clone();
        cx.emit(SessionEvent::InstallProgressChanged(progress));
    }

    fn handle_stream_item(&mut self, item: StreamItem, cx: &mut Context<Self>) {
        match item {
            StreamItem::Event(ev) => {
                cx.emit(SessionEvent::InstallLog(ev));
            }
            StreamItem::Done(outcome) => {
                let repo_phase_just_finished =
                    matches!(self.active_sysupgrade_phase, Some(SysupgradePhase::Repo));
                if matches!(outcome, ChildOutcome::Success)
                    && repo_phase_just_finished
                    && !self.sysupgrade_aur_targets.is_empty()
                {
                    self.active_sysupgrade_phase = Some(SysupgradePhase::Aur);
                    let targets = std::mem::take(&mut self.sysupgrade_aur_targets);
                    self.refresh_installed_state(cx);
                    self.start_sysupgrade_aur_build(targets, cx);
                    return;
                }
                match outcome {
                    ChildOutcome::Success => {
                        self.refresh_installed_state(cx);
                        self.start_updates_checker(cx);
                        self.set_progress(InstallProgress::Completed, cx);
                    }
                    ChildOutcome::Dismissed => self.set_progress(InstallProgress::Cancelled, cx),
                    ChildOutcome::NotFound => self.set_progress(
                        InstallProgress::Failed("install child not found".into()),
                        cx,
                    ),
                    ChildOutcome::Failed(message) => {
                        cx.emit(SessionEvent::InstallLog(InstallEvent::Log {
                            level: LogLevel::Error,
                            message: message.clone(),
                        }));
                        self.set_progress(InstallProgress::Failed(message), cx);
                    }
                }
                self.active_sysupgrade_phase = None;
                self.sysupgrade_aur_targets.clear();
            }
        }
    }

    pub(crate) fn spawn_install_subprocess(
        &mut self,
        name: String,
        source: PackageSource,
        approvals_b64: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };
        cx.emit(SessionEvent::InstallLogsOpened {
            kind: InstallKind::Install,
            source,
            name: name.clone(),
        });
        let (tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            crate::install::run_install_process(exe, name, tx, approvals_b64)
        });
        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    pub(crate) fn spawn_remove_subprocess(
        &mut self,
        name: String,
        source: PackageSource,
        cx: &mut Context<Self>,
    ) {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };
        cx.emit(SessionEvent::InstallLogsOpened {
            kind: InstallKind::Remove,
            source,
            name: name.clone(),
        });
        let (tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || crate::remove::run_remove_process(exe, name, tx));
        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    pub(crate) fn spawn_sysupgrade_subprocess(
        &mut self,
        fingerprint_file: String,
        approvals_b64: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };
        self.active_sysupgrade_phase = Some(SysupgradePhase::Repo);
        cx.emit(SessionEvent::InstallLogsOpened {
            kind: InstallKind::Upgrade,
            source: PackageSource::Repo,
            name: "system".to_string(),
        });
        let (tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            crate::upgrade::run_sysupgrade_process(exe, fingerprint_file, tx, approvals_b64)
        });
        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    fn start_sysupgrade_aur_build(&mut self, targets: Vec<String>, cx: &mut Context<Self>) {
        cx.emit(SessionEvent::InstallLogsOpened {
            kind: InstallKind::Upgrade,
            source: PackageSource::Aur,
            name: "system-aur".to_string(),
        });
        let (mut tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            let mut sink = ChannelSink { tx: tx.clone() };
            let result = crate::build::run_build(
                &targets,
                false,
                false,
                &mut sink,
                |_| crate::build::BuildDecision::Proceed,
                |_| true,
                None,
            );
            let outcome = match result {
                Ok(()) => ChildOutcome::Success,
                Err(e) => ChildOutcome::Failed(format!("{e:#}")),
            };
            let _ = tx.try_send(StreamItem::Done(outcome));
        });
        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    pub(crate) fn start_install(&mut self, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        let (name, source) = match &self.detail {
            DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source),
            _ => return,
        };
        self.set_progress(InstallProgress::Running, cx);
        let is_repo = matches!(source, PackageSource::Repo);

        self.pending_install = Some(PendingInstall {
            name: name.clone(),
            source,
            approvals_b64: None,
        });

        let (mut dry_tx, mut dry_rx) =
            futures::channel::mpsc::channel::<anyhow::Result<crate::question::QuestionSet>>(1);
        let name_for_dry_run = name.clone();
        std::thread::spawn(move || {
            let result = if is_repo {
                crate::dry_run::dry_run_for_repo_target(&name_for_dry_run)
            } else {
                crate::dry_run::dry_run_for_target(&name_for_dry_run)
            };
            let _ = dry_tx.try_send(result);
        });

        cx.spawn(async move |this, cx| {
            let Some(result) = dry_rx.next().await else {
                let _ = this.update(cx, |this, cx| {
                    this.pending_install.take();
                    this.set_progress(InstallProgress::Idle, cx);
                });
                return;
            };
            let _ = this.update(cx, |this, cx| match result {
                Ok(qs)
                    if !qs.conflicts.is_empty()
                        || !qs.providers.is_empty()
                        || qs.had_unsupported_question =>
                {
                    this.set_progress(InstallProgress::ConflictReview(qs.clone()), cx);
                    cx.emit(SessionEvent::ReviewRequired {
                        qs,
                        name: name.clone(),
                    });
                }
                Ok(_) => {
                    this.proceed_to_install_or_review(cx);
                }
                Err(err) => {
                    eprintln!("dry-run failed, proceeding with install: {err:#}");
                    this.proceed_to_install_or_review(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn start_remove(&mut self, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        let (name, source) = match &self.detail {
            DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source),
            _ => return,
        };
        if !is_installed(&self.alpm_handle, &name) {
            return;
        }
        self.set_progress(InstallProgress::Running, cx);
        self.spawn_remove_subprocess(name, source, cx);
    }

    pub(crate) fn start_sysupgrade_apply(
        &mut self,
        summary_bytes: Vec<u8>,
        approvals: crate::question::Approvals,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        let fingerprint_file = match crate::upgrade::write_fingerprint_file(&summary_bytes) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!("failed to write fingerprint file: {error}")),
                    cx,
                );
                return;
            }
        };
        match encode_approvals(&approvals) {
            Ok(b64) => {
                self.set_progress(InstallProgress::Running, cx);
                self.spawn_sysupgrade_subprocess(fingerprint_file, Some(b64), cx);
            }
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!("failed to encode approvals: {error}")),
                    cx,
                );
            }
        }
    }

    pub(crate) fn confirm_install(
        &mut self,
        approvals: crate::question::Approvals,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_install.as_mut() else {
            return;
        };
        match encode_approvals(&approvals) {
            Err(error) => {
                self.pending_install.take();
                self.set_progress(
                    InstallProgress::Failed(format!("failed to encode approvals: {error}")),
                    cx,
                );
            }
            Ok(b64) => {
                pending.approvals_b64 = Some(b64);
                self.proceed_to_install_or_review(cx);
            }
        }
    }

    fn proceed_to_install_or_review(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.pending_install else {
            return;
        };
        if matches!(pending.source, PackageSource::Aur) {
            self.start_pkgbuild_review(cx);
        } else {
            let pending = self.pending_install.take().unwrap();
            self.set_progress(InstallProgress::Running, cx);
            self.spawn_install_subprocess(pending.name, pending.source, pending.approvals_b64, cx);
        }
    }

    fn start_pkgbuild_review(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.pending_install else {
            return;
        };
        let target = pending.name.clone();
        self.set_progress(InstallProgress::PkgbuildReview, cx);

        let (mut tx, mut rx) =
            futures::channel::mpsc::channel::<anyhow::Result<Vec<PkgbuildDiff>>>(1);
        std::thread::spawn(move || {
            let result = prepare_pkgbuild_diffs(&[target]);
            let _ = tx.try_send(result);
        });

        cx.spawn(async move |this, cx| {
            let Some(result) = rx.next().await else {
                let _ = this.update(cx, |this, cx| {
                    this.cancel_pkgbuild_review(cx);
                });
                return;
            };
            let _ = this.update(cx, |this, cx| match result {
                Ok(diffs) if !diffs.is_empty() => {
                    cx.emit(SessionEvent::PkgbuildReviewRequired { diffs });
                }
                Ok(_) => {
                    this.confirm_pkgbuild_review(cx);
                }
                Err(err) => {
                    this.pending_install.take();
                    this.set_progress(
                        InstallProgress::Failed(format!("pkgbuild review failed: {err:#}")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(crate) fn confirm_pkgbuild_review(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_install.take() else {
            return;
        };
        self.set_progress(InstallProgress::Running, cx);
        self.spawn_install_subprocess(pending.name, pending.source, pending.approvals_b64, cx);
    }

    pub(crate) fn cancel_pkgbuild_review(&mut self, cx: &mut Context<Self>) {
        self.pending_install.take();
        self.set_progress(InstallProgress::Idle, cx);
    }

    pub(crate) fn cancel_install(&mut self, cx: &mut Context<Self>) {
        self.pending_install.take();
        self.set_progress(InstallProgress::Idle, cx);
    }

    pub(crate) fn reset_install(&mut self, cx: &mut Context<Self>) {
        self.cancel_install(cx);
    }

    fn refresh_installed_state(&mut self, cx: &mut Context<Self>) {
        if let Ok(config) = pacmanconf::Config::new()
            && let Ok(handle) = init_alpm(&config)
        {
            self.alpm_handle = handle;
        }
        self.installed_names = Arc::new(installed_names(&self.alpm_handle));
        for result in self.results.iter_mut() {
            result.installed = self.installed_names.contains(&result.name);
        }
        cx.emit(SessionEvent::SearchUpdated);
        self.refresh_detail_installed(cx);
        cx.notify();
    }

    pub(crate) fn start_db_lock_watcher(&mut self, cx: &mut Context<Self>) {
        let db_dir = pacmanconf::Config::new()
            .ok()
            .map(|c| std::path::PathBuf::from(c.db_path))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/pacman"));
        let (tx, mut rx) = futures::channel::mpsc::channel::<()>(16);
        crate::pacman_watch::spawn_db_lock_watcher(db_dir, tx);
        cx.spawn(async move |this, cx| {
            use futures::FutureExt as _;
            while let Some(()) = rx.next().await {
                while rx.next().now_or_never().is_some() {}
                cx.background_executor().timer(LOCK_DEBOUNCE).await;
                let _ = this.update(cx, |s, cx| s.refresh_installed_state(cx));
            }
        })
        .detach();
    }

    pub(crate) fn start_updates_checker(&mut self, cx: &mut Context<Self>) {
        if matches!(self.updates_state, UpdatesState::Loading) {
            return;
        }
        self.updates_state = UpdatesState::Loading;
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let reniced = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
            if reniced != 0 {
                eprintln!(
                    "[pakajo] failed to renice updates checker: {}",
                    std::io::Error::last_os_error()
                );
            }
            let result = crate::updates::pending_updates();
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| this.apply_updates_result(result, cx));
        })
        .detach();
    }

    fn apply_updates_result(
        &mut self,
        result: Result<
            Result<crate::updates::UpdatesFetch, anyhow::Error>,
            futures::channel::oneshot::Canceled,
        >,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Ok(fetch)) => {
                self.pending_updates = crate::updates::PendingUpdates {
                    repo: fetch.repo,
                    aur: fetch.aur,
                };
                self.updates_aur_error = fetch.aur_error;
                self.pending_count =
                    (self.pending_updates.repo.len() + self.pending_updates.aur.len()) as u32;
                self.updates_state = UpdatesState::Idle;
            }
            Ok(Err(e)) => {
                eprintln!("[pakajo] updates checker failed: {e:#}");
                self.updates_state = UpdatesState::Error(e.to_string());
            }
            Err(_) => {
                self.updates_state = UpdatesState::Error("updates check cancelled".into());
            }
        }
        cx.emit(SessionEvent::UpdatesAvailable(self.pending_count));
    }

    pub(crate) fn start_sysupgrade_preview(&mut self, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        if self.sysupgrade_preview_in_flight {
            return;
        }
        self.sysupgrade_preview_in_flight = true;
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let reniced = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
            if reniced != 0 {
                eprintln!(
                    "[pakajo] failed to renice sysupgrade preview: {}",
                    std::io::Error::last_os_error()
                );
            }
            let result = (|| -> anyhow::Result<crate::dry_run::SysupgradePreview> {
                let config = pacmanconf::Config::new().context("failed to read pacman config")?;
                let mut handle = crate::pacman::init_alpm_rootless(&config)?;
                handle
                    .syncdbs_mut()
                    .update(false)
                    .context("failed to refresh checkdb sync DBs rootless")?;
                let mut preview = crate::dry_run::compute_sysupgrade_preview(&mut handle, &config)?;
                let aur_names: Vec<String> = preview.aur.iter().map(|c| c.name.clone()).collect();
                if !aur_names.is_empty() {
                    match prepare_pkgbuild_diffs(&aur_names) {
                        Ok(diffs) => preview.pkgbuild_diffs = diffs,
                        Err(e) => eprintln!("[pakajo] pkgbuild diff computation failed: {e:#}"),
                    }
                }
                Ok(preview)
            })();
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.sysupgrade_preview_in_flight = false;
                let outcome = match result {
                    Ok(Ok(preview)) => {
                        this.sysupgrade_aur_targets =
                            preview.aur.iter().map(|c| c.name.clone()).collect();
                        Ok(preview)
                    }
                    Ok(Err(msg)) => Err(msg),
                    Err(_) => Err("sysupgrade preview cancelled".to_string()),
                };
                cx.emit(SessionEvent::SysupgradePreviewReady(outcome));
            });
        })
        .detach();
    }

    fn next_selected_index(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
        let max = len.checked_sub(1)?;
        let cur = current.unwrap_or(0);
        let next = (cur as i32 + delta).clamp(0, max as i32) as usize;
        if current == Some(next) {
            None
        } else {
            Some(next)
        }
    }

    pub(crate) fn on_search_change(&mut self, text: String, cx: &mut Context<Self>) {
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;

        if text.trim().is_empty() {
            self.clear(cx);
            self.search_state = SearchState::Idle;
            cx.notify();
            return;
        }

        self.search_state = SearchState::Searching;
        cx.notify();

        let search_engine = self.search_engine.clone();
        let local_index = self.local_index.clone();
        let installed = self.installed_names.clone();
        let group_index = self.group_index.clone();
        cx.spawn(async move |this, cx| {
            let still_valid = this
                .update(cx, |this, _cx| this.search_seq == seq)
                .unwrap_or(false);
            if !still_valid {
                return;
            }

            let outcome = cx
                .background_executor()
                .spawn(
                    async move {
                        execute_search_for(search_engine, local_index, group_index, installed, text)
                    },
                )
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.search_seq != seq {
                    return;
                }
                this.search_state = SearchState::Done;
                this.set_results(outcome, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn set_results(&mut self, results: Vec<SearchResult>, cx: &mut Context<Self>) {
        let prev_selected = self.selected_name().map(str::to_string);
        self.results = results;
        self.selected_index = if self.results.is_empty() {
            None
        } else {
            Some(0)
        };
        cx.emit(SessionEvent::SearchUpdated);
        if let Some(first) = self.results.first() {
            let unchanged = matches!(self.detail, DetailData::Loading | DetailData::Ready { .. })
                && prev_selected.as_deref() == Some(first.name.as_str());
            if !unchanged {
                let name = first.name.clone();
                let source = first.source;
                self.load_detail(name, source, cx);
            }
        }
    }

    pub(crate) fn select_by_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(result) = self.results.get(index) else {
            return;
        };
        let name = result.name.clone();
        let source = result.source;
        self.selected_index = Some(index);
        self.load_detail(name, source, cx);
    }

    pub(crate) fn select_delta(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(index) = Self::next_selected_index(self.results.len(), self.selected_index, delta)
        else {
            return;
        };
        cx.emit(SessionEvent::SearchUpdated);
        self.select_by_index(index, cx);
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.results.clear();
        self.selected_index = None;
        cx.emit(SessionEvent::SearchUpdated);
    }

    fn selected_name(&self) -> Option<&str> {
        self.selected_index
            .and_then(|i| self.results.get(i))
            .map(|r| r.name.as_str())
    }
}

pub(crate) fn execute_search_for(
    search_engine: Option<Arc<SearchEngine>>,
    local_index: Option<Arc<LocalIndex>>,
    group_index: Arc<Vec<(String, String)>>,
    installed: Arc<std::collections::HashSet<String>>,
    text: String,
) -> Vec<SearchResult> {
    let (Some(engine), Some(local)) = (search_engine.as_ref(), local_index.as_ref()) else {
        return Vec::new();
    };
    search::dispatch_search(engine, local, &installed, &text, &group_index)
}

pub(crate) fn begin_aur_sync_in_background(
    local_index: Arc<LocalIndex>,
    search_engine: Option<Arc<SearchEngine>>,
) {
    std::thread::spawn(move || {
        let reniced = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
        if reniced != 0 {
            eprintln!(
                "[pakajo] failed to renice aur sync worker: {}",
                std::io::Error::last_os_error()
            );
        }

        if let Some(age) = local_index.last_refreshed_age()
            && age < AUR_SYNC_MIN_INTERVAL
        {
            eprintln!(
                "[pakajo] skipping aur sync (last refresh {}h ago)",
                age.as_secs() / 3600
            );
            return;
        }

        let handle = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed to init alpm: {e:#}");
                return;
            }
        };

        match local_index.refresh(&handle) {
            Ok(crate::local_index::RefreshOutcome::NotModified) => {
                eprintln!("[pakajo] aur index up to date");
            }
            Ok(crate::local_index::RefreshOutcome::Updated {
                aur_count,
                repo_count,
                skipped,
            }) => {
                eprintln!(
                    "[pakajo] indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})"
                );
                if let Some(engine) = search_engine.as_ref() {
                    let _ = engine.ensure_fresh();
                }
            }
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed: {e:#}");
            }
        }
    });
}

struct ChannelSink {
    tx: futures::channel::mpsc::Sender<StreamItem>,
}

impl crate::events::InstallSink for ChannelSink {
    fn event(&mut self, event: InstallEvent) {
        let _ = self.tx.try_send(StreamItem::Event(event));
    }
}

#[cfg(test)]
mod tests {
    use super::PakajoSession;

    #[test]
    fn next_selected_index_empty_returns_none() {
        assert_eq!(PakajoSession::next_selected_index(0, None, 1), None);
    }

    #[test]
    fn next_selected_index_clamps_to_bounds() {
        assert_eq!(PakajoSession::next_selected_index(5, None, -3), Some(0));
        assert_eq!(PakajoSession::next_selected_index(5, None, 100), Some(4));
    }

    #[test]
    fn next_selected_index_noop_returns_none() {
        assert_eq!(PakajoSession::next_selected_index(5, Some(4), 1), None);
        assert_eq!(PakajoSession::next_selected_index(5, Some(0), -1), None);
    }

    #[test]
    fn next_selected_index_moves() {
        assert_eq!(PakajoSession::next_selected_index(5, Some(2), -1), Some(1));
        assert_eq!(PakajoSession::next_selected_index(5, Some(2), 1), Some(3));
    }
}
