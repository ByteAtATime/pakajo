use crate::db::{cache_path, engine_for};

const NAME_LIMIT: usize = 500;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let what = args.first().map(String::as_str).unwrap_or_default();
    let prefix = args.get(1).map(String::as_str).unwrap_or_default();
    match what {
        "installed" => print_installed(prefix),
        "available" => print_available(prefix),
        "any" => print_any(prefix),
        _ => anyhow::bail!("expected `installed`, `available` or `any`"),
    }
}

fn print_any(prefix: &str) -> anyhow::Result<()> {
    let mut names = installed_names(prefix)?;
    match indexed_names_with_prefix(prefix) {
        Some(remote) => names.extend(remote),
        None => names.extend(repo_names_with_prefix(prefix)?),
    }
    names.sort_unstable();
    names.dedup();
    names.truncate(NAME_LIMIT);
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn installed_names(prefix: &str) -> anyhow::Result<Vec<String>> {
    let handle = crate::pacman::handle()?;
    Ok(handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .filter(|n| n.starts_with(prefix))
        .collect())
}

fn print_installed(prefix: &str) -> anyhow::Result<()> {
    let mut names = installed_names(prefix)?;
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
    let sqlite = crate::db::PackageDb::db_path().ok()?;
    let db = crate::db::PackageDb::open(&sqlite).ok()?;
    match engine_for(&db, &cache_path(&sqlite)) {
        Err(_) => None,
        Ok(engine) if engine.is_empty() => None,
        Ok(engine) => Some(engine.complete_prefix(prefix, NAME_LIMIT)),
    }
}

fn repo_names_with_prefix(prefix: &str) -> anyhow::Result<Vec<String>> {
    let handle = crate::pacman::handle()?;
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
    use super::{indexed_names_with_prefix, run};
    use crate::db::PackageDb;
    use crate::db::{cache_path, engine_for};

    const PARITY_NAMES: &[&str] = &[
        "alpha",
        "alpine",
        "al1",
        "al2",
        "al3",
        "al4",
        "beta",
        "firefox",
        "firefox-bin",
        "firefox-esr",
        "gamma",
    ];

    fn seed_names(path: &std::path::Path, names: &[&str]) {
        let conn = rusqlite::Connection::open(path).expect("seed conn");
        for name in names {
            conn.execute(
                "INSERT INTO packages (name, source) VALUES (?1, 'aur')",
                rusqlite::params![name],
            )
            .expect("seed package");
        }
    }

    fn sql_prefix_names(path: &std::path::Path, prefix: &str, limit: usize) -> Vec<String> {
        let conn = rusqlite::Connection::open(path).expect("read conn");
        let mut stmt = conn
            .prepare("SELECT name FROM packages WHERE name LIKE ?1 || '%' ORDER BY name LIMIT ?2")
            .expect("prepare");
        stmt.query_map(rusqlite::params![prefix, limit as i64], |row| {
            row.get::<_, String>(0)
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect")
    }

    #[test]
    fn run_rejects_missing_and_unknown_kinds() {
        assert!(run(&[]).is_err());
        assert!(run(&["bogus".to_string()]).is_err());
    }

    #[test]
    fn engine_prefix_matches_sql_parity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        seed_names(&sqlite_path, PARITY_NAMES);
        drop(db);
        let db = PackageDb::open(&sqlite_path).expect("reopen");
        let engine = engine_for(&db, &cache_path(&sqlite_path)).expect("engine");

        for (prefix, limit) in [
            ("al", 3),
            ("al", 10),
            ("firefox", 10),
            ("FIRE", 10),
            ("gamma", 10),
            ("zz", 10),
            ("", 4),
        ] {
            assert_eq!(
                engine.complete_prefix(prefix, limit),
                sql_prefix_names(&sqlite_path, prefix, limit),
                "engine and SQL must agree for prefix {prefix:?} limit {limit}"
            );
        }
        assert_eq!(
            engine.complete_prefix("al", 3),
            vec!["al1".to_string(), "al2".to_string(), "al3".to_string(),]
        );
    }

    struct CacheHomeGuard {
        prior: Option<String>,
    }

    impl CacheHomeGuard {
        fn point_at(dir: &tempfile::TempDir) -> Self {
            let prior = std::env::var("XDG_CACHE_HOME").ok();
            unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };
            Self { prior }
        }
    }

    impl Drop for CacheHomeGuard {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => unsafe { std::env::set_var("XDG_CACHE_HOME", value) },
                None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
            }
        }
    }

    #[test]
    fn indexed_glue_falls_back_on_empty_and_answers_when_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = CacheHomeGuard::point_at(&dir);
        let sqlite = PackageDb::db_path().expect("db path");
        PackageDb::open(&sqlite).expect("open");
        assert_eq!(indexed_names_with_prefix("fire"), None);
        seed_names(&sqlite, &["firefox", "firefox-bin", "gamma"]);
        assert_eq!(
            indexed_names_with_prefix("fire"),
            Some(vec!["firefox".to_string(), "firefox-bin".to_string()])
        );
        assert_eq!(indexed_names_with_prefix("zz"), Some(Vec::new()));
    }
}
