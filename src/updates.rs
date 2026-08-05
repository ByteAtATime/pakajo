use anyhow::Context as _;

use crate::pacman;
use crate::upgrade::AurUpgradeCandidate;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoUpgrade {
    pub name: String,
    pub old: String,
    pub new: String,
    pub download_size: i64,
    pub repo: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingUpdates {
    pub repo: Vec<RepoUpgrade>,
    pub aur: Vec<AurUpgradeCandidate>,
}

#[derive(Debug, Clone)]
pub struct UpdatesFetch {
    pub repo: Vec<RepoUpgrade>,
    pub aur: Vec<AurUpgradeCandidate>,
    pub aur_error: Option<String>,
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
        if let Some(sync) = pacman::find_pkg(handle, pkg.name())
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

pub fn pending_updates() -> anyhow::Result<UpdatesFetch> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = pacman::init_alpm_rootless(&config)?;
    handle
        .syncdbs_mut()
        .update(false)
        .context("failed to refresh sync DBs rootless")?;
    let repo = compute_repo_upgrades(&handle, &config)?;
    let aur_client = crate::aur::AurClient::new();
    let (aur, aur_error) = match crate::upgrade::compute_aur_upgrades(&handle, &aur_client) {
        Ok(v) => (v, None),
        Err(e) => {
            let msg = format!("{e:#}");
            eprintln!("[pakajo] aur update check failed, showing repo updates only: {msg}");
            (Vec::new(), Some(msg))
        }
    };
    Ok(UpdatesFetch {
        repo,
        aur,
        aur_error,
    })
}
