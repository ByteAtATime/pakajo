use alpm::{Alpm, SigLevel};
use anyhow::Context as _;

pub mod lock;
pub mod snapshot;

fn parse_siglevel(sig_strings: &[String]) -> SigLevel {
    if sig_strings.is_empty() {
        return SigLevel::USE_DEFAULT;
    }

    let mut level = SigLevel::empty();

    for s in sig_strings {
        let flag = match s.as_str() {
            "Never" => SigLevel::NONE,
            "Optional" => SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL,
            "Required" => SigLevel::PACKAGE,
            "TrustedOnly" => SigLevel::empty(),
            "TrustAll" => SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK,

            "DatabaseOptional" => SigLevel::DATABASE | SigLevel::DATABASE_OPTIONAL,
            "DatabaseRequired" => SigLevel::DATABASE,
            "DatabaseTrustedOnly" => SigLevel::empty(),
            "DatabaseTrustAll" => SigLevel::DATABASE_MARGINAL_OK | SigLevel::DATABASE_UNKNOWN_OK,

            "PackageOptional" => SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL,
            "PackageRequired" => SigLevel::PACKAGE,
            "PackageTrustedOnly" => SigLevel::empty(),
            "PackageTrustAll" => SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK,

            other => SigLevel::from_name(other).unwrap_or_else(SigLevel::empty),
        };

        level = level.union(flag);
    }

    if level.is_empty() {
        SigLevel::USE_DEFAULT
    } else {
        level
    }
}

pub fn init_alpm(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
    let mut handle = Alpm::new(config.root_dir.as_str(), config.db_path.as_str())
        .context("failed to initialize alpm")?;
    alpm_utils::configure_alpm(&mut handle, config)
        .map_err(|e| anyhow::anyhow!("failed to configure alpm: {e}"))?;
    Ok(handle)
}

pub fn init_alpm_at(
    config: &pacmanconf::Config,
    root: &str,
    db_path: &str,
    cache_dirs: &[String],
) -> anyhow::Result<Alpm> {
    let mut handle = Alpm::new(root, db_path)?;
    handle.set_architectures(config.architecture.iter())?;
    for dir in cache_dirs {
        handle.add_cachedir(dir.as_str())?;
    }
    let inherited = parse_siglevel(&config.sig_level);
    for repo in &config.repos {
        let mut level = if repo.sig_level.is_empty() {
            inherited
        } else {
            parse_siglevel(&repo.sig_level)
        };
        level.remove(
            SigLevel::DATABASE
                | SigLevel::DATABASE_OPTIONAL
                | SigLevel::DATABASE_MARGINAL_OK
                | SigLevel::DATABASE_UNKNOWN_OK,
        );
        let db = handle.register_syncdb_mut(repo.name.clone(), level)?;
        db.set_servers(repo.servers.iter())?;
    }
    Ok(handle)
}

pub fn init_alpm_rootless(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
    let checkdb = crate::build::cache_root()?.join("checkdb");
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
            let checkdb = crate::build::cache_root()?.join("checkdb");
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

pub fn find_pkg<'a>(handle: &'a Alpm, name: &str) -> Option<&'a alpm::Package> {
    handle.syncdbs().iter().find_map(|db| db.pkg(name).ok())
}
