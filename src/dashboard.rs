use std::collections::HashSet;

use anyhow::Context as _;

#[derive(Clone, Debug, Default)]
pub struct DashboardSnapshot {
    pub installed_total: u64,
    pub repo_count: u64,
    pub aur_count: u64,
    pub total_bytes: i64,
    pub repo_bytes: i64,
    pub aur_bytes: i64,
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
) -> DashboardSnapshot {
    let mut snapshot = DashboardSnapshot::default();
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
    }
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
    let snapshot = compute_dashboard_snapshot(&handle, &foreign);
    Ok((foreign, snapshot))
}
