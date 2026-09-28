use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

pub struct PackageDb {
    read: std::sync::Mutex<rusqlite::Connection>,
    write: std::sync::Mutex<rusqlite::Connection>,
}

impl PackageDb {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let write = rusqlite::Connection::open(path)?;
        write.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=1000;")?;
        apply_schema(&write)?;
        let read = rusqlite::Connection::open(path)?;
        apply_schema(&read)?;
        Ok(Self {
            read: std::sync::Mutex::new(read),
            write: std::sync::Mutex::new(write),
        })
    }

    pub fn db_path() -> anyhow::Result<std::path::PathBuf> {
        let dir = crate::utils::cache_root()?;
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join("aur-meta.sqlite"))
    }

    pub fn is_populated(&self) -> bool {
        let conn = self.read.lock().expect("read connection poisoned");
        conn.query_row("SELECT EXISTS(SELECT 1 FROM packages LIMIT 1)", [], |row| {
            row.get::<_, bool>(0)
        })
        .unwrap_or(false)
    }

    pub fn get_meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.read.lock().expect("read connection poisoned");
        meta_get(&conn, key)
    }

    pub fn last_refreshed_age(&self) -> Option<Duration> {
        let raw = self.get_meta("last_refreshed").ok().flatten()?;
        let parsed: u64 = raw.parse().ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        Some(Duration::from_secs(now.as_secs().saturating_sub(parsed)))
    }
}

fn hash_file_stats(entries: &[(u128, u64)]) -> u64 {
    let mut hash = 14695981039346656037u64;
    for (nanos, len) in entries {
        for byte in nanos.to_le_bytes().iter().chain(len.to_le_bytes().iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1099511628211u64);
        }
    }
    hash
}

pub fn db_cache_fingerprint(db: &PackageDb) -> u64 {
    let Some(main) = main_db_path(db) else {
        return u64::MAX;
    };
    let mut siblings = Vec::with_capacity(3);
    siblings.push(main.clone());
    for suffix in ["-wal", "-shm"] {
        let mut name = main.clone().into_os_string();
        name.push(suffix);
        siblings.push(std::path::PathBuf::from(name));
    }
    let mut stats = Vec::with_capacity(3);
    for path in siblings {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return u64::MAX,
        };
        let modified = match meta.modified() {
            Ok(modified) => modified,
            Err(_) => return u64::MAX,
        };
        let nanos = match modified.duration_since(UNIX_EPOCH) {
            Ok(age) => age.as_nanos(),
            Err(_) => return u64::MAX,
        };
        stats.push((nanos, meta.len()));
    }
    hash_file_stats(&stats)
}

fn main_db_path(db: &PackageDb) -> Option<std::path::PathBuf> {
    let conn = db.read.lock().expect("read connection poisoned");
    let mut stmt = conn.prepare("PRAGMA database_list").ok()?;
    let mut rows = stmt.query([]).ok()?;
    while let Ok(Some(row)) = rows.next() {
        let name: String = row.get(1).ok()?;
        if name != "main" {
            continue;
        }
        let file: String = row.get(2).ok()?;
        if file.is_empty() {
            return None;
        }
        return Some(std::path::PathBuf::from(file));
    }
    None
}

fn apply_schema(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS packages (\
           name TEXT PRIMARY KEY, source TEXT, repo TEXT, version TEXT, description TEXT,\
           num_votes INTEGER, popularity REAL, last_update INTEGER, package_base TEXT,\
           url TEXT, out_of_date INTEGER, maintainer TEXT, license TEXT, depends TEXT,\
           make_depends TEXT, check_depends TEXT, opt_depends TEXT, conflicts TEXT,\
           provides TEXT, keywords TEXT);\
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
    Ok(())
}

fn meta_get(conn: &rusqlite::Connection, key: &str) -> anyhow::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?)
}

fn meta_set(conn: &rusqlite::Connection, key: &str, value: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?,?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

pub(super) const PKG_COLS: [&str; 20] = [
    "name",
    "source",
    "repo",
    "version",
    "description",
    "num_votes",
    "popularity",
    "last_update",
    "package_base",
    "url",
    "out_of_date",
    "maintainer",
    "license",
    "depends",
    "make_depends",
    "check_depends",
    "opt_depends",
    "conflicts",
    "provides",
    "keywords",
];

pub(super) fn pkg_insert_sql() -> String {
    let cols = PKG_COLS.join(",");
    let placeholders = (0..PKG_COLS.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    format!("INSERT OR REPLACE INTO packages ({cols}) VALUES ({placeholders})")
}

pub(super) fn pkg_upsert_sql() -> String {
    let cols = PKG_COLS.join(",");
    let placeholders = (0..PKG_COLS.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let updates = PKG_COLS
        .iter()
        .map(|col| format!("{col}=excluded.{col}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO packages ({cols}) VALUES ({placeholders}) \
         ON CONFLICT(name) DO UPDATE SET {updates}"
    )
}

pub(super) fn join_list(list: &[String]) -> String {
    list.join("\n")
}

pub(super) fn split_list(value: Option<String>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(text) => text
            .split('\n')
            .filter(|entry| !entry.is_empty())
            .map(|entry| clean_text(entry.to_string()))
            .collect(),
    }
}

pub(super) fn clean_text(value: String) -> String {
    crate::color::strip_controls(&value).into_owned()
}

pub mod detail;
pub mod fetch;
pub mod query;
pub mod sync;

pub use fetch::{AUR_META_URL, DecompressedDump, FetchOutcome, fetch};
pub use query::PackageRow;
pub use sync::RefreshOutcome;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_file_stats_matches_independent_vector() {
        let entries = [
            (1700000000000000000u128, 12345u64),
            (987654321u128, 67890u64),
        ];
        assert_eq!(hash_file_stats(&entries), 11988688413877841901u64);
    }

    #[test]
    fn meta_round_trip_upserts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("raw conn");

        assert_eq!(index.get_meta("foo").expect("get_meta"), None);
        meta_set(&conn, "foo", "bar").expect("meta_set");
        assert_eq!(
            index.get_meta("foo").expect("get_meta"),
            Some("bar".to_string())
        );
        meta_set(&conn, "foo", "baz").expect("meta_set upsert");
        assert_eq!(
            index.get_meta("foo").expect("get_meta"),
            Some("baz".to_string())
        );
    }

    #[test]
    fn last_refreshed_age_is_none_when_unset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = PackageDb::open(&dir.path().join("aur-meta.sqlite")).expect("open");
        assert!(index.last_refreshed_age().is_none());
    }

    #[test]
    fn last_refreshed_age_is_none_for_garbage_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?, ?)",
            rusqlite::params!["last_refreshed", "not-a-number"],
        )
        .expect("seed");
        drop(conn);
        assert!(index.last_refreshed_age().is_none());
    }
}
