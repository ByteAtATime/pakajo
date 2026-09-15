use alpm::{Alpm, SigLevel};
use anyhow::Context as _;

const INITIAL_SIG_LEVEL: SigLevel = SigLevel::PACKAGE.union(SigLevel::DATABASE);

const FALLBACK_SIG_LEVEL: SigLevel = SigLevel::PACKAGE
    .union(SigLevel::PACKAGE_OPTIONAL)
    .union(SigLevel::DATABASE)
    .union(SigLevel::DATABASE_OPTIONAL);

pub struct SigLevels {
    pub default: SigLevel,
    pub local_file: SigLevel,
    pub remote_file: SigLevel,
}

pub fn local_file_siglevel(handle: &Alpm) -> SigLevel {
    with_fallback(handle.local_file_siglevel())
}

fn with_fallback(level: SigLevel) -> SigLevel {
    if level == SigLevel::USE_DEFAULT {
        return FALLBACK_SIG_LEVEL;
    }
    level
}

pub(super) fn apply_sig_levels(handle: &Alpm, config: &pacmanconf::Config) -> anyhow::Result<()> {
    let levels = resolve(
        &config.sig_level,
        &config.local_file_sig_level,
        &config.remote_file_sig_level,
    )?;
    apply(handle, &levels)
}

pub(super) fn init_handle(handle: &mut Alpm, config: &pacmanconf::Config) -> anyhow::Result<()> {
    let levels = resolve(
        &config.sig_level,
        &config.local_file_sig_level,
        &config.remote_file_sig_level,
    )?;
    apply(handle, &levels)?;
    for repo in &config.repos {
        let (level, mask) = apply_directives(SigLevel::USE_DEFAULT, &repo.sig_level)?;
        let db = handle
            .register_syncdb_mut(
                repo.name.clone(),
                merge_sig_level(levels.default, level, mask),
            )
            .with_context(|| format!("failed to register sync database {}", repo.name))?;
        db.set_servers(repo.servers.iter())?;
    }
    Ok(())
}

fn apply(handle: &Alpm, levels: &SigLevels) -> anyhow::Result<()> {
    handle
        .set_default_siglevel(levels.default)
        .context("failed to set default siglevel")?;
    handle
        .set_local_file_siglevel(levels.local_file)
        .context("failed to set local file siglevel")?;
    handle
        .set_remote_file_siglevel(levels.remote_file)
        .context("failed to set remote file siglevel")?;
    Ok(())
}

fn resolve(
    sig_level: &[String],
    local_file: &[String],
    remote_file: &[String],
) -> anyhow::Result<SigLevels> {
    let (default, _) = apply_directives(INITIAL_SIG_LEVEL, sig_level)?;
    let (local, local_mask) = apply_directives(SigLevel::USE_DEFAULT, local_file)?;
    let (remote, remote_mask) = apply_directives(SigLevel::USE_DEFAULT, remote_file)?;
    Ok(SigLevels {
        default,
        local_file: merge_sig_level(default, local, local_mask),
        remote_file: merge_sig_level(default, remote, remote_mask),
    })
}

fn apply_directives(
    mut level: SigLevel,
    directives: &[String],
) -> anyhow::Result<(SigLevel, SigLevel)> {
    let mut mask = SigLevel::empty();

    for directive in directives {
        let (packages, databases, keyword) =
            if let Some(keyword) = directive.strip_prefix("Package") {
                (true, false, keyword)
            } else if let Some(keyword) = directive.strip_prefix("Database") {
                (false, true, keyword)
            } else {
                (true, true, directive.as_str())
            };

        let sides = [
            (
                packages,
                SigLevel::PACKAGE,
                SigLevel::PACKAGE_OPTIONAL,
                SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK,
            ),
            (
                databases,
                SigLevel::DATABASE,
                SigLevel::DATABASE_OPTIONAL,
                SigLevel::DATABASE_MARGINAL_OK | SigLevel::DATABASE_UNKNOWN_OK,
            ),
        ];

        for (enabled, check, optional, trust) in sides {
            if !enabled {
                continue;
            }
            let (set, unset) = match keyword {
                "Never" => (SigLevel::empty(), check),
                "Optional" => (check | optional, SigLevel::empty()),
                "Required" => (check, optional),
                "TrustedOnly" => (SigLevel::empty(), trust),
                "TrustAll" => (trust, SigLevel::empty()),
                other => anyhow::bail!("invalid SigLevel directive: {other}"),
            };
            level = level.union(set).difference(unset);
            mask |= set | unset;
        }

        level = level.difference(SigLevel::USE_DEFAULT);
    }

    Ok((level, mask))
}

fn merge_sig_level(base: SigLevel, over: SigLevel, mask: SigLevel) -> SigLevel {
    if mask.is_empty() {
        return over;
    }
    over.intersection(mask).union(base.difference(mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn resolve_levels(sig: &[&str], local: &[&str], remote: &[&str]) -> SigLevels {
        resolve(&list(sig), &list(local), &list(remote)).unwrap()
    }

    fn bits(flags: &[SigLevel]) -> SigLevel {
        flags
            .iter()
            .fold(SigLevel::empty(), |acc, flag| acc.union(*flag))
    }

    const STOCK: &[&str] = &["Required", "DatabaseOptional"];

    #[test]
    fn stock_config_makes_local_signatures_optional() {
        let levels = resolve_levels(STOCK, &["Optional"], &[]);
        assert_eq!(
            levels.local_file,
            bits(&[
                SigLevel::PACKAGE,
                SigLevel::PACKAGE_OPTIONAL,
                SigLevel::DATABASE,
                SigLevel::DATABASE_OPTIONAL
            ])
        );
    }

    #[test]
    fn required_local_files_reject_unsigned_packages() {
        let levels = resolve_levels(STOCK, &["Required"], &[]);
        assert!(levels.local_file.contains(SigLevel::PACKAGE));
        assert!(!levels.local_file.contains(SigLevel::PACKAGE_OPTIONAL));
    }

    #[test]
    fn local_overrides_inherit_untouched_sig_level_bits() {
        let levels = resolve_levels(&["Optional", "TrustAll"], &["Required"], &[]);
        assert!(levels.local_file.contains(SigLevel::PACKAGE_MARGINAL_OK));
        assert!(!levels.local_file.contains(SigLevel::PACKAGE_OPTIONAL));
    }

    #[test]
    fn never_clears_bits_set_by_earlier_directives() {
        let levels = resolve_levels(&["Optional", "Never"], &[], &[]);
        assert_eq!(
            levels.default,
            bits(&[SigLevel::PACKAGE_OPTIONAL, SigLevel::DATABASE_OPTIONAL])
        );
    }

    #[test]
    fn bare_config_defaults_to_strict_checks() {
        let levels = resolve_levels(&[], &[], &[]);
        assert_eq!(levels.default, INITIAL_SIG_LEVEL);
        assert_eq!(levels.local_file, SigLevel::USE_DEFAULT);
        assert_eq!(levels.remote_file, SigLevel::USE_DEFAULT);
    }

    #[test]
    fn prefixes_scope_directives_to_one_side() {
        let levels = resolve_levels(STOCK, &["DatabaseRequired"], &["PackageTrustAll"]);
        assert_eq!(
            levels.local_file,
            SigLevel::PACKAGE.union(SigLevel::DATABASE)
        );
        assert!(levels.remote_file.contains(SigLevel::PACKAGE_MARGINAL_OK));
        assert!(!levels.remote_file.contains(SigLevel::DATABASE_MARGINAL_OK));
    }

    #[test]
    fn repeated_directives_accumulate_in_order() {
        let levels = resolve_levels(&["Required", "Optional"], &[], &[]);
        assert_eq!(
            levels.default,
            bits(&[
                SigLevel::PACKAGE,
                SigLevel::PACKAGE_OPTIONAL,
                SigLevel::DATABASE,
                SigLevel::DATABASE_OPTIONAL
            ])
        );
    }

    #[test]
    fn unknown_directives_are_rejected() {
        assert!(resolve(&list(&["Bogus"]), &[], &[]).is_err());
        assert!(resolve(&[], &list(&["Package"]), &[]).is_err());
    }
}
