use super::commands::alpm_handle;

const NAME_LIMIT: usize = 500;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let what = args.first().map(String::as_str).unwrap_or_default();
    let prefix = args.get(1).map(String::as_str).unwrap_or_default();
    match what {
        "installed" => print_installed(prefix),
        "available" => print_available(prefix),
        _ => anyhow::bail!("expected `installed` or `available`"),
    }
}

fn print_installed(prefix: &str) -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let mut names: Vec<String> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort_unstable();
    names.truncate(NAME_LIMIT);
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn print_available(prefix: &str) -> anyhow::Result<()> {
    let names = match indexed_names_with_prefix(prefix) {
        Some(names) => names,
        None => repo_names_with_prefix(prefix)?,
    };
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn indexed_names_with_prefix(prefix: &str) -> Option<Vec<String>> {
    let path = crate::local_index::LocalIndex::db_path().ok()?;
    let index = crate::local_index::LocalIndex::open(&path).ok()?;
    if !index.is_populated() {
        return None;
    }
    index.names_with_prefix(prefix, NAME_LIMIT).ok()
}

fn repo_names_with_prefix(prefix: &str) -> anyhow::Result<Vec<String>> {
    let handle = alpm_handle()?;
    let mut names: Vec<String> = handle
        .syncdbs()
        .iter()
        .flat_map(|db| db.pkgs().iter())
        .map(|p| p.name().to_string())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort_unstable();
    names.dedup();
    names.truncate(NAME_LIMIT);
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn run_rejects_missing_and_unknown_kinds() {
        assert!(run(&[]).is_err());
        assert!(run(&["bogus".to_string()]).is_err());
    }
}
