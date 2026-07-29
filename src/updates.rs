use anyhow::Context as _;

use crate::pacman;

pub(crate) fn rootless_upgradable_count() -> anyhow::Result<usize> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = pacman::init_alpm_rootless(&config)?;
    handle
        .syncdbs_mut()
        .update(false)
        .context("failed to refresh sync DBs rootless")?;
    let local = handle.localdb();
    let ignore_pkgs: std::collections::HashSet<&str> =
        config.ignore_pkg.iter().map(String::as_str).collect();
    let ignore_groups: std::collections::HashSet<&str> =
        config.ignore_group.iter().map(String::as_str).collect();
    let mut count = 0usize;
    for pkg in local.pkgs().iter() {
        if ignore_pkgs.contains(pkg.name()) {
            continue;
        }
        if pkg.groups().iter().any(|g| ignore_groups.contains(g)) {
            continue;
        }
        if let Some(sync) = pacman::find_pkg(&handle, pkg.name())
            && alpm::vercmp(sync.version().to_string(), pkg.version().to_string())
                == std::cmp::Ordering::Greater
        {
            count += 1;
        }
    }
    Ok(count)
}
