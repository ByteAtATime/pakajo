use alpm::Alpm;
use anyhow::Context as _;

pub mod lock;
pub mod snapshot;

mod siglevel;
pub use siglevel::local_file_siglevel;
use siglevel::{apply_sig_levels, init_handle};

pub fn config() -> anyhow::Result<pacmanconf::Config> {
    pacmanconf::Config::new().context("failed to read pacman config")
}

pub fn db_path() -> std::path::PathBuf {
    config().map_or_else(
        |_| std::path::PathBuf::from("/var/lib/pacman"),
        |config| std::path::PathBuf::from(config.db_path),
    )
}

pub fn handle() -> anyhow::Result<Alpm> {
    handle_with_config(&config()?)
}

pub fn handle_with_config(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
    let mut handle = Alpm::new(config.root_dir.as_str(), config.db_path.as_str())
        .context("failed to initialize alpm")?;
    let system_hookdirs: Vec<String> = handle
        .hookdirs()
        .iter()
        .map(|dir| dir.to_string())
        .collect();
    alpm_utils::configure_alpm(&mut handle, config)
        .map_err(|e| anyhow::anyhow!("failed to configure alpm: {e}"))?;
    let config_hookdirs: Vec<String> = handle
        .hookdirs()
        .iter()
        .map(|dir| dir.to_string())
        .collect();
    let mut hookdirs: Vec<String> = Vec::new();
    for dir in &system_hookdirs {
        if !std::path::Path::new(dir).exists() {
            continue;
        }
        push_hookdir(&mut hookdirs, dir);
    }
    for dir in &config_hookdirs {
        push_hookdir(&mut hookdirs, dir);
    }
    handle
        .set_hookdirs(hookdirs.iter().map(String::as_str))
        .context("failed to set hookdirs")?;
    apply_sig_levels(&handle, config)?;
    Ok(handle)
}

fn trim_slash(path: &str) -> &str {
    path.trim_end_matches('/')
}

fn push_hookdir(hookdirs: &mut Vec<String>, dir: &str) {
    let known = hookdirs
        .iter()
        .any(|known| trim_slash(known) == trim_slash(dir));
    if !known {
        hookdirs.push(dir.to_string());
    }
}

pub(crate) fn init_alpm_at(
    config: &pacmanconf::Config,
    root: &str,
    db_path: &str,
    cache_dirs: &[String],
) -> anyhow::Result<Alpm> {
    let mut handle = Alpm::new(root, db_path)?;
    init_handle(&mut handle, config)?;
    handle.set_architectures(config.architecture.iter())?;
    for dir in cache_dirs {
        handle.add_cachedir(dir.as_str())?;
    }
    Ok(handle)
}

pub fn handle_rootless_with_config(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
    let checkdb = crate::utils::cache_root()?.join("checkdb");
    std::fs::create_dir_all(&checkdb).context("creating checkdb dir")?;
    let local_link = checkdb.join("local");
    let expected_local = std::path::Path::new(&config.db_path).join("local");
    let needs_link = match std::fs::read_link(&local_link) {
        Ok(target) => target != expected_local,
        Err(_) => true,
    };
    if needs_link {
        let _ = std::fs::remove_file(&local_link);
        std::os::unix::fs::symlink(&expected_local, &local_link)
            .with_context(|| format!("symlinking local db -> {}", expected_local.display()))?;
    }
    let checkdb_str = checkdb.to_string_lossy().to_string();
    let mut handle = init_alpm_at(config, "/", &checkdb_str, &config.cache_dir)
        .context("initializing rootless alpm handle")?;
    handle
        .set_gpgdir(config.gpg_dir.as_str())
        .context("forwarding gpgdir")?;
    Ok(handle)
}

static CHECKDB_REFRESH_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_is_stale(path: &std::path::Path) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return true;
    };
    modified
        .elapsed()
        .map(|e| e.as_secs() >= 60)
        .unwrap_or(false)
}

pub fn refresh_sync_dbs_rootless(handle: &mut Alpm) -> anyhow::Result<()> {
    let _guard = CHECKDB_REFRESH_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match handle.syncdbs_mut().update(false) {
        Ok(_) => Ok(()),
        Err(alpm::Error::HandleLock) => {
            let lock_path = lock::db_lck_path(handle.dbpath());
            let checkdb = crate::utils::cache_root()?.join("checkdb");
            if lock_path.parent() != Some(checkdb.as_path()) {
                anyhow::bail!(
                    "refusing to remove non-checkdb lock at {}",
                    lock_path.display()
                );
            }
            if !lock_is_stale(&lock_path) {
                anyhow::bail!(
                    "checkdb lock at {} is fresh, another pakajo refresh may be in progress",
                    lock_path.display()
                );
            }
            let age = std::fs::metadata(&lock_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|e| e.as_secs())
                .unwrap_or(0);
            match std::fs::remove_file(&lock_path) {
                Ok(()) => {
                    eprintln!(
                        "[pakajo] removed stale checkdb lock at {} (held for {}s)",
                        lock_path.display(),
                        age
                    );
                    handle
                        .syncdbs_mut()
                        .update(false)
                        .context("failed to refresh sync DBs rootless after stale lock removal")?;
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    anyhow::bail!(
                        "could not acquire checkdb lock and no stale lock file was present at {} (check that the directory is writable and not full)",
                        lock_path.display()
                    )
                }
                Err(e) => Err(e).with_context(|| {
                    format!("removing stale checkdb lock at {}", lock_path.display())
                }),
            }
        }
        Err(e) => Err(anyhow::Error::new(e).context("failed to refresh sync DBs rootless")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_with_config_preserves_system_hookdir() {
        let root = tempfile::tempdir().unwrap();
        let syshooks = root.path().join("usr/share/libalpm/hooks");
        std::fs::create_dir_all(&syshooks).unwrap();
        let probe = root.path().join("etc/pacman.d/hooks");
        std::fs::create_dir_all(&probe).unwrap();
        let db = root.path().join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut config = pacmanconf::Config::default();
        config.root_dir = root.path().to_string_lossy().into_owned();
        config.db_path = db.to_string_lossy().into_owned();
        config.hook_dir = vec![probe.to_string_lossy().into_owned()];
        config.cache_dir = vec![root.path().join("cache").to_string_lossy().into_owned()];
        config.gpg_dir = root.path().join("gnupg").to_string_lossy().into_owned();
        config.log_file = root
            .path()
            .join("pacman.log")
            .to_string_lossy()
            .into_owned();
        config.architecture = vec!["x86_64".to_string()];
        let handle = handle_with_config(&config).unwrap();
        let dirs: Vec<String> = handle
            .hookdirs()
            .iter()
            .map(|dir| dir.to_string())
            .collect();
        assert_eq!(
            dirs,
            vec![
                format!("{}/", syshooks.display()),
                format!("{}/", probe.display()),
            ]
        );
    }

    #[test]
    fn handle_with_config_skips_missing_system_hookdir() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("db");
        std::fs::create_dir_all(&db).unwrap();
        let mut config = pacmanconf::Config::default();
        config.root_dir = root.path().to_string_lossy().into_owned();
        config.db_path = db.to_string_lossy().into_owned();
        config.gpg_dir = root.path().join("gnupg").to_string_lossy().into_owned();
        config.log_file = root
            .path()
            .join("pacman.log")
            .to_string_lossy()
            .into_owned();
        config.architecture = vec!["x86_64".to_string()];
        let handle = handle_with_config(&config).unwrap();
        let dirs: Vec<String> = handle
            .hookdirs()
            .iter()
            .map(|dir| dir.to_string())
            .collect();
        assert_eq!(dirs, Vec::<String>::new());
    }
}
