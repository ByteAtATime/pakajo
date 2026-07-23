use crate::{
    aur::AurClient,
    local_index::LocalIndex,
    package::{Package, PackageSource, installed_names, is_installed},
    pacman::{find_pkg, init_alpm},
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider},
};
use alpm::Alpm;
use anyhow::Context as _;
use gpui::*;
use std::{sync::Arc, time::Duration};

const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum DetailData {
    None,
    Loading,
    Ready { pkg: Package, installed: bool },
    Error(String),
}

pub(crate) enum SessionEvent {
    DetailUpdated,
}

impl EventEmitter<SessionEvent> for PakajoSession {}

pub(crate) struct PakajoSession {
    pub(crate) alpm_handle: Alpm,
    pub(crate) aur_client: Arc<AurClient>,
    pub(crate) repo_index: Arc<RepoSearchIndex>,
    pub(crate) local_index: Option<Arc<LocalIndex>>,
    pub(crate) installed_names: Arc<std::collections::HashSet<String>>,
    pub(crate) detail: DetailData,
    detail_seq: u64,
}

impl PakajoSession {
    pub(crate) fn new(alpm_handle: Alpm, aur_client: AurClient) -> Self {
        let repo_index = Arc::new(RepoSearchIndex::from_alpm(&alpm_handle));
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
        let aur_client = Arc::new(aur_client);
        if let Some(index) = &local_index {
            begin_aur_sync_in_background(index.clone());
        }
        Self {
            alpm_handle,
            aur_client,
            repo_index,
            local_index,
            installed_names,
            detail: DetailData::None,
            detail_seq: 0,
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
}

pub(crate) fn execute_search_for(
    local_index: Option<Arc<LocalIndex>>,
    repo_index: &Arc<RepoSearchIndex>,
    aur_client: &Arc<AurClient>,
    installed: Arc<std::collections::HashSet<String>>,
    text: &str,
) -> search::SearchOutcome {
    let repo_provider = RepoSearchProvider::new(repo_index.clone());
    let aur_provider = AurSearchProvider::new(aur_client.clone());
    search::dispatch_search(local_index, &repo_provider, &aur_provider, &installed, text)
}

pub(crate) fn begin_aur_sync_in_background(local_index: Arc<LocalIndex>) {
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
            }
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed: {e:#}");
            }
        }
    });
}
