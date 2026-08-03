use alpm::{Alpm, SigLevel};
use anyhow::Context as _;

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
    verify_db_sigs: bool,
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
        if !verify_db_sigs {
            level.remove(
                SigLevel::DATABASE
                    | SigLevel::DATABASE_OPTIONAL
                    | SigLevel::DATABASE_MARGINAL_OK
                    | SigLevel::DATABASE_UNKNOWN_OK,
            );
        }
        let db = handle.register_syncdb_mut(repo.name.clone(), level)?;
        db.set_servers(repo.servers.iter())?;
    }
    Ok(handle)
}

pub(crate) fn init_alpm_rootless(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
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
    let mut handle = init_alpm_at(config, "/", &checkdb_str, &config.cache_dir, false)
        .context("initializing rootless alpm handle")?;
    handle
        .set_gpgdir(config.gpg_dir.as_str())
        .context("forwarding gpgdir")?;
    Ok(handle)
}

pub fn find_pkg<'a>(handle: &'a Alpm, name: &str) -> Option<&'a alpm::Package> {
    handle.syncdbs().iter().find_map(|db| db.pkg(name).ok())
}
