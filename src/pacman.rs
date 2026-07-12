use alpm::{Alpm, SigLevel};

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
    init_alpm_at(config, &config.root_dir, &config.db_path, &config.cache_dir)
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
    for repo in &config.repos {
        let db = handle.register_syncdb_mut(repo.name.clone(), parse_siglevel(&repo.sig_level))?;
        db.set_servers(repo.servers.iter())?;
    }
    Ok(handle)
}

pub fn find_pkg<'a>(handle: &'a Alpm, name: &str) -> Option<&'a alpm::Package> {
    handle.syncdbs().iter().find_map(|db| db.pkg(name).ok())
}
