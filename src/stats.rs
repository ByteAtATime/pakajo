use alpm::Alpm;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub struct InstalledStats {
    pub total: usize,
    pub repo_count: usize,
    pub aur_count: usize,
    pub total_size_bytes: i64,
}

impl InstalledStats {
    pub fn from_alpm(handle: &Alpm) -> Self {
        let sync_names: HashSet<String> = handle
            .syncdbs()
            .iter()
            .flat_map(|db| db.pkgs().iter())
            .map(|p| p.name().to_string())
            .collect();

        let mut total = 0usize;
        let mut repo_count = 0usize;
        let mut aur_count = 0usize;
        let mut total_size_bytes = 0i64;

        for pkg in handle.localdb().pkgs().iter() {
            total += 1;
            total_size_bytes += pkg.isize();
            if sync_names.contains(pkg.name()) {
                repo_count += 1;
            } else {
                aur_count += 1;
            }
        }

        if sync_names.is_empty() && total > 0 {
            eprintln!("[pakajo] stats: no sync DBs found; all packages classified as AUR");
        }

        Self {
            total,
            repo_count,
            aur_count,
            total_size_bytes,
        }
    }
}
