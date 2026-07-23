use crate::{
    aur::AurClient,
    local_index::LocalIndex,
    package::{Package, PackageSource, installed_names, is_installed},
    pacman::{find_pkg, init_alpm},
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult},
};
use alpm::Alpm;
use anyhow::Context as _;
use gpui::*;
use std::{sync::Arc, time::Duration};

const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);
const LIVE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum DetailData {
    None,
    Loading,
    Ready { pkg: Package, installed: bool },
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SearchState {
    Idle,
    Searching,
    Done,
}

pub(crate) enum SessionEvent {
    DetailUpdated,
    SearchUpdated,
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
    pub(crate) results: Vec<SearchResult>,
    pub(crate) selected_index: Option<usize>,
    search_seq: u64,
    pub(crate) search_state: SearchState,
    aur_error: Option<String>,
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
            results: Vec::new(),
            selected_index: None,
            search_seq: 0,
            search_state: SearchState::Idle,
            aur_error: None,
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
            self.aur_error = None;
            self.search_state = SearchState::Idle;
            cx.notify();
            return;
        }

        self.search_state = SearchState::Searching;
        self.aur_error = None;
        cx.notify();

        let repo_index = self.repo_index.clone();
        let aur_client = self.aur_client.clone();
        let local_index = self.local_index.clone();
        let installed = self.installed_names.clone();
        let debounce = if self
            .local_index
            .as_ref()
            .is_some_and(|i| i.is_populated())
        {
            Duration::ZERO
        } else {
            LIVE_DEBOUNCE
        };
        cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }

            let still_valid = this
                .update(cx, |this, _cx| this.search_seq == seq)
                .unwrap_or(false);
            if !still_valid {
                return;
            }

            let outcome = cx
                .background_executor()
                .spawn(async move {
                    execute_search_for(local_index, &repo_index, &aur_client, installed, &text)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.search_seq != seq {
                    return;
                }
                this.aur_error = outcome.aur_error;
                this.search_state = SearchState::Done;
                if let Some(err) = &this.aur_error {
                    eprintln!("  aur: {err}");
                }
                this.set_results(outcome.results, cx);
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
            let unchanged = matches!(
                self.detail,
                DetailData::Loading | DetailData::Ready { .. }
            ) && prev_selected.as_deref() == Some(first.name.as_str());
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
        let Some(index) =
            Self::next_selected_index(self.results.len(), self.selected_index, delta)
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
