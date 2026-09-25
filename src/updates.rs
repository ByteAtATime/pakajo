use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alpm_utils::DbListExt;
use anyhow::Context as _;

use crate::pacman;
use crate::upgrade::{AurUpgradeCandidate, DevelSource};

pub const REPO_REVALIDATE_AFTER: Duration = Duration::from_secs(60 * 60);
pub const DEVEL_RECHECK_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpdatesCache {
    pub checked_at: u64,
    pub devel_checked_at: u64,
    pub repo: Vec<RepoUpgrade>,
    pub aur: Vec<AurUpgradeCandidate>,
    pub devel: Vec<String>,
}

impl UpdatesCache {
    pub fn repo_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.checked_at) >= REPO_REVALIDATE_AFTER.as_secs()
    }

    pub fn devel_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.devel_checked_at) >= DEVEL_RECHECK_AFTER.as_secs()
    }
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub fn store(cache: &UpdatesCache) {
    let dir = match crate::utils::cache_root() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[pakajo] failed to write updates cache: {e:#}");
            return;
        }
    };
    match store_inner(cache, &dir) {
        Ok(path) => eprintln!(
            "[pakajo] updates cache written to {} (repo={} aur={} devel={})",
            path.display(),
            cache.repo.len(),
            cache.aur.len(),
            cache.devel.len()
        ),
        Err(e) => eprintln!("[pakajo] failed to write updates cache: {e:#}"),
    }
}

fn store_inner(cache: &UpdatesCache, dir: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join("updates.json");
    let tmp = dir.join("updates.json.tmp");
    let bytes = serde_json::to_vec(cache).context("failed to serialize updates cache")?;
    std::fs::write(&tmp, bytes).context("failed to write updates cache temp file")?;
    std::fs::rename(&tmp, &target).context("failed to rename updates cache into place")?;
    Ok(target)
}

fn load_from(dir: &std::path::Path) -> Option<UpdatesCache> {
    let bytes = match std::fs::read(dir.join("updates.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("[pakajo] failed to read updates cache: {e}");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(cache) => Some(cache),
        Err(e) => {
            eprintln!("[pakajo] failed to parse updates cache: {e}");
            None
        }
    }
}

pub fn load_cached() -> Option<UpdatesCache> {
    let dir = match crate::utils::cache_root() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[pakajo] failed to read updates cache: {e:#}");
            return None;
        }
    };
    load_from(&dir)
}

pub fn localdb_unchanged_since(checked_at: u64) -> bool {
    let Ok(config) = pacman::config() else {
        return false;
    };
    let db_path = std::path::PathBuf::from(config.db_path);
    let mtime_secs = std::fs::metadata(db_path.join("local"))
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    match mtime_secs {
        Some(mtime_secs) => mtime_secs <= checked_at,
        None => false,
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoUpgrade {
    pub name: String,
    pub old: String,
    pub new: String,
    pub download_size: i64,
    pub repo: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PendingUpdates {
    pub repo: Vec<RepoUpgrade>,
    pub aur: Vec<AurUpgradeCandidate>,
}

#[derive(Debug, Clone)]
pub struct UpdatesFetch {
    pub repo: Vec<RepoUpgrade>,
    pub aur: Vec<AurUpgradeCandidate>,
    pub devel_names: Vec<String>,
    pub aur_error: Option<String>,
    pub devel_live: bool,
}

pub fn compute_repo_upgrades(
    handle: &alpm::Alpm,
    config: &pacmanconf::Config,
) -> anyhow::Result<Vec<RepoUpgrade>> {
    let ignore_pkgs: std::collections::HashSet<&str> =
        config.ignore_pkg.iter().map(String::as_str).collect();
    let ignore_groups: std::collections::HashSet<&str> =
        config.ignore_group.iter().map(String::as_str).collect();
    let mut repo: Vec<RepoUpgrade> = Vec::new();
    for pkg in handle.localdb().pkgs().iter() {
        if ignore_pkgs.contains(pkg.name()) {
            continue;
        }
        if pkg.groups().iter().any(|g| ignore_groups.contains(g)) {
            continue;
        }
        if let Some(sync) = handle.syncdbs().pkg(pkg.name()).ok()
            && alpm::vercmp(sync.version().to_string(), pkg.version().to_string())
                == std::cmp::Ordering::Greater
        {
            repo.push(RepoUpgrade {
                name: pkg.name().to_string(),
                old: pkg.version().to_string(),
                new: sync.version().to_string(),
                download_size: sync.download_size(),
                repo: sync.db().map(|d| d.name().to_string()).unwrap_or_default(),
            });
        }
    }
    repo.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(repo)
}

pub fn pending_updates(devel: DevelSource) -> anyhow::Result<UpdatesFetch> {
    let devel_live = matches!(devel, DevelSource::Live);
    let config = pacman::config()?;
    let mut handle = pacman::handle_rootless_with_config(&config)?;
    pacman::refresh_sync_dbs_rootless(&mut handle)?;
    let repo = compute_repo_upgrades(&handle, &config)?;
    let aur_client = crate::aur::AurClient::new();
    let (aur, devel_names, aur_error) =
        match crate::upgrade::compute_aur_upgrades(&handle, &aur_client, devel) {
            Ok((v, devel)) => (v, devel, None),
            Err(e) => {
                let msg = format!("{e:#}");
                eprintln!("[pakajo] aur update check failed, showing repo updates only: {msg}");
                (Vec::new(), Vec::new(), Some(msg))
            }
        };
    Ok(UpdatesFetch {
        repo,
        aur,
        devel_names,
        aur_error,
        devel_live,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pakajo-test-{}-{name}", std::process::id()))
    }

    fn sample_cache(checked_at: u64) -> UpdatesCache {
        UpdatesCache {
            checked_at,
            devel_checked_at: checked_at,
            repo: Vec::new(),
            aur: Vec::new(),
            devel: Vec::new(),
        }
    }

    #[test]
    fn load_from_returns_none_for_corrupt_file() {
        let dir = unique_temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.json"), b"\xff\xfe{not json").unwrap();
        assert!(load_from(&dir).is_none());
    }

    #[test]
    fn load_from_returns_none_for_missing_file() {
        let dir = unique_temp_dir("missing");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_from(&dir).is_none());
    }

    #[test]
    fn staleness_predicates_respect_age_boundaries() {
        let cache = sample_cache(1000);
        assert!(!cache.repo_stale(1000 + 3599));
        assert!(cache.repo_stale(1000 + 3600));
        assert!(!cache.devel_stale(1000 + 6 * 3600 - 1));
        assert!(cache.devel_stale(1000 + 6 * 3600));
        assert!(!cache.repo_stale(999));
        assert!(!cache.devel_stale(999));
    }
}
