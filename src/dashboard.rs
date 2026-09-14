use std::collections::{HashMap, HashSet};

use anyhow::Context as _;

#[derive(Clone, Debug, Default)]
pub struct OptdepEntry {
    pub name: String,
    pub requesters: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct RecentPkg {
    pub name: String,
    pub version: String,
    pub age: String,
}

#[derive(Clone, Debug, Default)]
pub struct DashboardSnapshot {
    pub installed_total: u64,
    pub repo_count: u64,
    pub aur_count: u64,
    pub total_bytes: i64,
    pub repo_bytes: i64,
    pub aur_bytes: i64,
    pub optdeps: Vec<OptdepEntry>,
    pub optdep_total: usize,
    pub recent: Vec<RecentPkg>,
}

#[derive(Clone, Debug)]
pub enum DashboardMessage {
    SnapshotReady {
        seq: u64,
        foreign: HashSet<String>,
        snapshot: DashboardSnapshot,
    },
    LoadFailed {
        seq: u64,
        error: String,
    },
}

pub fn compute_dashboard_snapshot(
    handle: &alpm::Alpm,
    foreign: &HashSet<String>,
    now: i64,
) -> DashboardSnapshot {
    let mut snapshot = DashboardSnapshot::default();
    let mut installed: HashSet<String> = HashSet::new();
    let mut optdep_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut dated: Vec<(String, String, i64)> = Vec::new();
    for pkg in handle.localdb().pkgs().iter() {
        let size = pkg.isize();
        snapshot.installed_total += 1;
        snapshot.total_bytes += size;
        if foreign.contains(pkg.name()) {
            snapshot.aur_count += 1;
            snapshot.aur_bytes += size;
        } else {
            snapshot.repo_count += 1;
            snapshot.repo_bytes += size;
        }
        let requester = pkg.name().to_string();
        installed.insert(requester.clone());
        for opt in pkg.optdepends().iter() {
            let dep_name = opt.name();
            if dep_name.is_empty() {
                continue;
            }
            let reason = opt
                .desc()
                .filter(|d| !d.is_empty())
                .unwrap_or("")
                .to_string();
            let requesters = optdep_map.entry(dep_name.to_string()).or_default();
            if requesters
                .iter()
                .any(|(existing, _)| *existing == requester)
            {
                continue;
            }
            requesters.push((requester.clone(), reason));
        }
        if let Some(date) = pkg.install_date().filter(|date| *date != 0) {
            dated.push((requester.clone(), pkg.version().as_str().to_string(), date));
        }
    }
    let mut optdeps: Vec<(String, Vec<(String, String)>)> = optdep_map
        .into_iter()
        .filter(|(name, _)| !installed.contains(name))
        .collect();
    optdeps.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    snapshot.optdep_total = optdeps.len();
    snapshot.optdeps = optdeps
        .into_iter()
        .take(5)
        .map(|(name, requesters)| OptdepEntry { name, requesters })
        .collect();
    dated.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    snapshot.recent = dated
        .into_iter()
        .take(5)
        .map(|(name, version, installed_at)| {
            let elapsed = now.saturating_sub(installed_at).max(0) as u64;
            RecentPkg {
                name,
                version,
                age: crate::utils::humanize_age(elapsed),
            }
        })
        .collect();
    snapshot
}

pub fn gather_dashboard() -> anyhow::Result<(HashSet<String>, DashboardSnapshot)> {
    let handle = match pacmanconf::Config::new()
        .context("failed to read pacman config")
        .and_then(|cfg| crate::pacman::init_alpm(&cfg))
    {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("[pakajo] dashboard refresh failed: {e:#}");
            return Err(e);
        }
    };
    let foreign: HashSet<String> = crate::package::foreign_names(&handle).into_iter().collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let snapshot = compute_dashboard_snapshot(&handle, &foreign, now);
    Ok((foreign, snapshot))
}
