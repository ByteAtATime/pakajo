use crate::{
    aur::AurClient,
    local_index::LocalIndex,
    package::installed_names,
    pacman::init_alpm,
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider},
};
use alpm::Alpm;
use anyhow::Context as _;
use std::{sync::Arc, time::Duration};

const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

pub(crate) struct PakajoSession {
    pub(crate) alpm_handle: Alpm,
    pub(crate) aur_client: Arc<AurClient>,
    pub(crate) repo_index: Arc<RepoSearchIndex>,
    pub(crate) local_index: Option<Arc<LocalIndex>>,
    pub(crate) installed_names: Arc<std::collections::HashSet<String>>,
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
