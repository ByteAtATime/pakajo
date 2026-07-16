use std::io::BufRead as _;
use std::time::Duration;

use anyhow::Context as _;

pub const AUR_META_URL: &str = "https://aur.archlinux.org/packages-meta-ext-v1.json.gz";

const FETCH_TIMEOUT: Duration = Duration::from_secs(600);

pub enum FetchOutcome {
    NotModified,
    Updated(DecompressedDump),
}

pub struct DecompressedDump {
    resp: ureq::http::Response<ureq::Body>,
    last_modified: String,
}

impl DecompressedDump {
    pub fn reader(&mut self) -> impl std::io::BufRead + use<'_> {
        std::io::BufReader::new(flate2::read::GzDecoder::new(
            self.resp.body_mut().as_reader(),
        ))
    }

    pub fn last_modified(&self) -> &str {
        &self.last_modified
    }
}

pub fn fetch(url: &str, if_modified_since: Option<&str>) -> anyhow::Result<FetchOutcome> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();

    let mut request = agent.get(url);
    if let Some(since) = if_modified_since {
        request = request.header("If-Modified-Since", since);
    }

    let response = request
        .call()
        .with_context(|| format!("failed to fetch {url}"))?;

    if response.status().as_u16() == 304 {
        return Ok(FetchOutcome::NotModified);
    }

    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|value| value.to_str().ok())
        .map(|raw| raw.to_string())
        .with_context(|| format!("{url} response is missing a Last-Modified header"))?;

    Ok(FetchOutcome::Updated(DecompressedDump {
        resp: response,
        last_modified,
    }))
}

pub enum RefreshOutcome {
    NotModified,
    Updated {
        aur_count: usize,
        repo_count: usize,
        skipped: usize,
    },
}

pub struct LocalIndex {
    read: std::sync::Mutex<rusqlite::Connection>,
    write: std::sync::Mutex<rusqlite::Connection>,
}

impl LocalIndex {
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
        let cache = match std::env::var("XDG_CACHE_HOME") {
            Ok(xdg) => std::path::PathBuf::from(xdg),
            Err(_) => {
                let home = std::env::var("HOME")
                    .context("no cache directory: set XDG_CACHE_HOME or HOME")?;
                std::path::PathBuf::from(home).join(".cache")
            }
        };
        let dir = cache.join("pakajo");
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join("aur-meta.sqlite"))
    }

    #[allow(dead_code)]
    pub fn is_populated(&self) -> bool {
        let conn = self.read.lock().expect("read connection poisoned");
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM packages LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn row_count(&self) -> anyhow::Result<i64> {
        let conn = self.read.lock().expect("read connection poisoned");
        Ok(conn.query_row("SELECT COUNT(*) FROM packages", [], |row| {
            row.get::<_, i64>(0)
        })?)
    }

    #[allow(dead_code)]
    pub fn get_meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.read.lock().expect("read connection poisoned");
        meta_get(&conn, key)
    }

    #[allow(dead_code)]
    fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.write.lock().expect("write connection poisoned");
        meta_set(&conn, key, value)
    }

    pub fn refresh(&self, handle: &alpm::Alpm) -> anyhow::Result<RefreshOutcome> {
        let mut conn = self.write.lock().expect("write connection poisoned");

        let last_modified = meta_get(&conn, "last_modified")?;
        let mut dump = match fetch(AUR_META_URL, last_modified.as_deref())? {
            FetchOutcome::NotModified => return Ok(RefreshOutcome::NotModified),
            FetchOutcome::Updated(dump) => dump,
        };

        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM packages; DELETE FROM packages_fts;")?;

        let mut pkg_stmt = tx.prepare(
            "INSERT OR REPLACE INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
        )?;
        let mut fts_stmt = tx.prepare(
            "INSERT INTO packages_fts \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
        )?;

        let mut aur_count: usize = 0;
        let mut skipped: usize = 0;
        for line in dump.reader().lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let payload = trimmed
                .trim_start_matches('[')
                .trim_end_matches([',', ']'])
                .trim();
            if payload.is_empty() {
                continue;
            }
            match serde_json::from_str::<crate::aur::AurInfo>(payload) {
                Ok(info) => {
                    pkg_stmt.execute(rusqlite::params![
                        &info.name,
                        "aur",
                        "aur",
                        &info.version,
                        &info.description,
                        info.num_votes as i64,
                        info.popularity,
                        info.last_modified,
                        &info.package_base,
                    ])?;
                    fts_stmt.execute(rusqlite::params![
                        &info.name,
                        &info.description,
                        "aur",
                        "aur",
                        &info.version,
                        info.num_votes as i64,
                        info.popularity,
                        info.last_modified,
                        &info.package_base,
                    ])?;
                    aur_count += 1;
                }
                Err(_) => skipped += 1,
            }
        }

        if skipped > (aur_count + skipped) / 100 {
            let total = aur_count + skipped;
            let msg = format!("too many malformed AUR rows: {skipped} of {total}");
            drop(pkg_stmt);
            drop(fts_stmt);
            drop(tx);
            let _ = meta_set(&conn, "last_error", &msg);
            return Err(anyhow::Error::msg(msg));
        }

        let null_votes: Option<i64> = None;
        let null_popularity: Option<f64> = None;
        let null_base: Option<&str> = None;
        let mut repo_count: usize = 0;
        for db in handle.syncdbs().iter() {
            let repo = db.name();
            for pkg in db.pkgs().iter() {
                pkg_stmt.execute(rusqlite::params![
                    pkg.name(),
                    "repo",
                    repo,
                    pkg.version().as_str(),
                    pkg.desc(),
                    &null_votes,
                    &null_popularity,
                    pkg.build_date(),
                    &null_base,
                ])?;
                fts_stmt.execute(rusqlite::params![
                    pkg.name(),
                    pkg.desc(),
                    "repo",
                    repo,
                    pkg.version().as_str(),
                    &null_votes,
                    &null_popularity,
                    pkg.build_date(),
                    &null_base,
                ])?;
                repo_count += 1;
            }
        }

        drop(pkg_stmt);
        drop(fts_stmt);
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        meta_set(&conn, "last_modified", dump.last_modified())?;
        meta_set(&conn, "last_refreshed", &now.to_string())?;
        meta_set(&conn, "aur_count", &aur_count.to_string())?;
        meta_set(&conn, "repo_count", &repo_count.to_string())?;
        meta_set(&conn, "last_error", "")?;

        Ok(RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        })
    }
}

fn apply_schema(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS packages (\
           name TEXT PRIMARY KEY, source TEXT, repo TEXT, version TEXT, description TEXT,\
           num_votes INTEGER, popularity REAL, last_update INTEGER, package_base TEXT);\
         CREATE VIRTUAL TABLE IF NOT EXISTS packages_fts USING fts5(\
           name, description,\
           source UNINDEXED, repo UNINDEXED, version UNINDEXED, num_votes UNINDEXED,\
           popularity UNINDEXED, last_update UNINDEXED, package_base UNINDEXED,\
           tokenize = 'unicode61 remove_diacritics 2');\
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
    Ok(())
}

fn meta_get(conn: &rusqlite::Connection, key: &str) -> anyhow::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?",
            [key],
            |row| row.get::<_, String>(0),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn live_conditional_fetch_round_trip() {
        let first = fetch(AUR_META_URL, None).expect("initial fetch should succeed");
        let last_modified = match first {
            FetchOutcome::Updated(dump) => dump.last_modified().to_string(),
            FetchOutcome::NotModified => panic!("initial fetch should return Updated"),
        };

        let second =
            fetch(AUR_META_URL, Some(&last_modified)).expect("conditional fetch should succeed");
        match second {
            FetchOutcome::NotModified => {}
            FetchOutcome::Updated(_) => {
                panic!("conditional fetch should return NotModified for a fresh Last-Modified");
            }
        }
    }

    #[test]
    fn open_creates_schema_and_starts_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let check = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).unwrap();
        let names: Vec<String> = check
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(names.contains(&"packages".to_string()), "packages table missing");
        assert!(
            names.contains(&"packages_fts".to_string()),
            "packages_fts table missing"
        );
        assert!(names.contains(&"meta".to_string()), "meta table missing");

        assert!(!index.is_populated());
        assert_eq!(index.row_count().expect("row_count"), 0);
    }

    #[test]
    fn meta_round_trip_upserts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        assert_eq!(index.get_meta("foo").expect("get_meta"), None);
        index.set_meta("foo", "bar").expect("set_meta");
        assert_eq!(
            index.get_meta("foo").expect("get_meta"),
            Some("bar".to_string())
        );
        index.set_meta("foo", "baz").expect("set_meta upsert");
        assert_eq!(
            index.get_meta("foo").expect("get_meta"),
            Some("baz".to_string())
        );
    }

    #[test]
    #[ignore]
    fn live_refresh_indexes_aur_and_repo() {
        let config = pacmanconf::Config::new().expect("pacman config");
        let handle = crate::pacman::init_alpm(&config).expect("alpm handle");
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let outcome = index.refresh(&handle).expect("refresh");
        let aur_count = match outcome {
            RefreshOutcome::Updated { aur_count, .. } => aur_count,
            RefreshOutcome::NotModified => panic!("first refresh should return Updated"),
        };
        assert!(aur_count > 80_000, "aur_count too low: {aur_count}");
        assert!(index.is_populated());
    }
}
