use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        conn.query_row("SELECT EXISTS(SELECT 1 FROM packages LIMIT 1)", [], |row| {
            row.get::<_, bool>(0)
        })
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

    pub fn last_refreshed_age(&self) -> Option<Duration> {
        let raw = self.get_meta("last_refreshed").ok().flatten()?;
        let parsed: u64 = raw.parse().ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        Some(Duration::from_secs(now.as_secs().saturating_sub(parsed)))
    }

    #[allow(dead_code)]
    fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.write.lock().expect("write connection poisoned");
        meta_set(&conn, key, value)
    }

    pub fn search(&self, pattern: &str, limit: i64) -> anyhow::Result<Vec<PackageRow>> {
        if pattern.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn.prepare(
            "SELECT p.name, p.description, p.source, p.repo, p.version, p.num_votes, \
             p.popularity, p.last_update, p.package_base, p.keywords \
             FROM packages_fts \
             JOIN packages p ON p.rowid = packages_fts.rowid \
             WHERE packages_fts MATCH ?1 \
             ORDER BY bm25(packages_fts, 10.0, 1.0, 5.0) \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit], row_to_package)?;
        rows.collect::<rusqlite::Result<Vec<PackageRow>>>()
            .map_err(anyhow::Error::from)
    }

    pub fn search_name_prefix_ranked(
        &self,
        needle: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<PackageRow>> {
        let needle: String = needle
            .chars()
            .filter_map(|c| c.is_alphanumeric().then(|| c.to_ascii_lowercase()))
            .collect();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("name:{needle}*");
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn.prepare(
            "SELECT p.name, p.description, p.source, p.repo, p.version, p.num_votes, \
             p.popularity, p.last_update, p.package_base, p.keywords \
             FROM packages_fts \
             JOIN packages p ON p.rowid = packages_fts.rowid \
             WHERE packages_fts MATCH ?1 \
             ORDER BY (p.name = ?2) DESC, (p.name LIKE ?2 || '%') DESC, length(p.name) ASC, p.name ASC \
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![&pattern, &needle, limit], row_to_package)?;
        rows.collect::<rusqlite::Result<Vec<PackageRow>>>()
            .map_err(anyhow::Error::from)
    }

    pub fn search_recent_repo_prefix(
        &self,
        prefix: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<PackageRow>> {
        let pattern = format!("{}*", prefix);
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn.prepare(
            "SELECT p.name, p.description, p.source, p.repo, p.version, p.num_votes, \
             p.popularity, p.last_update, p.package_base, p.keywords \
             FROM packages p \
             WHERE p.source = 'repo' AND p.name GLOB ?1 \
             ORDER BY p.last_update DESC \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit], row_to_package)?;
        rows.collect::<rusqlite::Result<Vec<PackageRow>>>()
            .map_err(anyhow::Error::from)
    }

    pub fn detail(&self, name: &str) -> anyhow::Result<Option<crate::aur::AurInfo>> {
        use rusqlite::OptionalExtension;
        let conn = self.read.lock().expect("read connection poisoned");
        let json: Option<String> = conn
            .query_row(
                "SELECT detail_json FROM packages WHERE name = ?1 AND detail_json IS NOT NULL",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn put_detail(&self, info: &crate::aur::AurInfo) -> anyhow::Result<()> {
        let conn = self.write.lock().expect("write connection poisoned");
        let json = serde_json::to_string(info)?;
        conn.execute(
            "INSERT OR REPLACE INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base,detail_json,keywords) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                &info.name,
                "aur",
                "aur",
                &info.version,
                &info.description,
                info.num_votes as i64,
                info.popularity,
                info.last_modified,
                &info.package_base,
                &json,
                &info.keywords.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn refresh(&self, handle: &alpm::Alpm) -> anyhow::Result<RefreshOutcome> {
        let mut conn = self.write.lock().expect("write connection poisoned");

        let last_modified = meta_get(&conn, "last_modified")?;
        let mut dump = match fetch(AUR_META_URL, last_modified.as_deref())? {
            FetchOutcome::NotModified => return Ok(RefreshOutcome::NotModified),
            FetchOutcome::Updated(dump) => dump,
        };

        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM packages;")?;

        let mut pkg_stmt = tx.prepare(
            "INSERT OR REPLACE INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base,detail_json,keywords) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        )?;

        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, dump.reader())?;

        if fail_loud(aur_count, skipped) {
            let total = aur_count + skipped;
            let msg = format!("too many malformed AUR rows: {skipped} of {total}");
            drop(pkg_stmt);
            drop(tx);
            let _ = meta_set(&conn, "last_error", &msg);
            return Err(anyhow::Error::msg(msg));
        }

        let null_votes: Option<i64> = None;
        let null_popularity: Option<f64> = None;
        let null_base: Option<&str> = None;
        let null_detail: Option<&str> = None;
        let null_keywords: Option<&str> = None;
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
                    &null_detail,
                    &null_keywords,
                ])?;
                repo_count += 1;
            }
        }

        drop(pkg_stmt);
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

pub struct PackageRow {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub repo: Option<String>,
    pub version: String,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
    pub last_update: Option<i64>,
    #[allow(dead_code)]
    pub package_base: Option<String>,
    pub keywords: Vec<String>,
}

fn row_to_package(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRow> {
    let keywords_raw: Option<String> = row.get(9)?;
    Ok(PackageRow {
        name: row.get(0)?,
        description: row.get(1)?,
        source: row.get(2)?,
        repo: row.get(3)?,
        version: row.get(4)?,
        num_votes: row.get(5)?,
        popularity: row.get(6)?,
        last_update: row.get(7)?,
        package_base: row.get(8)?,
        keywords: keywords_raw
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
    })
}

fn index_aur_rows(
    pkg_stmt: &mut rusqlite::Statement,
    reader: impl std::io::BufRead,
) -> anyhow::Result<(usize, usize)> {
    let mut aur_count: usize = 0;
    let mut skipped: usize = 0;
    for line in reader.lines() {
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
                    payload,
                    &info.keywords.join(" "),
                ])?;
                aur_count += 1;
            }
            Err(_) => skipped += 1,
        }
    }
    Ok((aur_count, skipped))
}

fn fail_loud(aur: usize, skipped: usize) -> bool {
    skipped > (aur + skipped) / 100
}

fn apply_schema(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS packages (\
           name TEXT PRIMARY KEY, source TEXT, repo TEXT, version TEXT, description TEXT,\
           num_votes INTEGER, popularity REAL, last_update INTEGER, package_base TEXT,\
           detail_json TEXT, keywords TEXT);\
         CREATE VIRTUAL TABLE IF NOT EXISTS packages_fts USING fts5(\
           name, description, keywords,\
           content='packages', content_rowid=rowid,\
           tokenize = 'unicode61 remove_diacritics 2');\
         CREATE TRIGGER IF NOT EXISTS packages_ai AFTER INSERT ON packages BEGIN \
           INSERT INTO packages_fts(rowid, name, description, keywords) \
             VALUES (new.rowid, new.name, new.description, new.keywords); \
         END;\
         CREATE TRIGGER IF NOT EXISTS packages_ad AFTER DELETE ON packages BEGIN \
           INSERT INTO packages_fts(packages_fts, rowid, name, description, keywords) \
             VALUES('delete', old.rowid, old.name, old.description, old.keywords); \
         END;\
         CREATE TRIGGER IF NOT EXISTS packages_au AFTER UPDATE ON packages BEGIN \
           INSERT INTO packages_fts(packages_fts, rowid, name, description, keywords) \
             VALUES('delete', old.rowid, old.name, old.description, old.keywords); \
           INSERT INTO packages_fts(rowid, name, description, keywords) \
             VALUES (new.rowid, new.name, new.description, new.keywords); \
         END;\
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aur::AurInfo;

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

    const PKG_INSERT_SQL: &str = "INSERT OR REPLACE INTO packages \
         (name,source,repo,version,description,num_votes,popularity,last_update,package_base,detail_json,keywords) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?)";

    fn aur_json(id: u64, name: &str) -> String {
        format!(
            r#"{{"ID":{id},"Name":"{name}","PackageBaseID":{id},"PackageBase":"{name}","Version":"1.0-1","NumVotes":0,"Popularity":0.0,"FirstSubmitted":0,"LastModified":0}}"#
        )
    }

    #[test]
    fn index_aur_rows_tolerates_delimiters_and_skips_malformed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let input = format!(
            "[\n\
             \n\
             [{},\n\
             {},\n\
             {}]\n\
             {{ garbage }}\n\
             ]",
            aur_json(1, "alpha"),
            aur_json(2, "beta"),
            aur_json(3, "gamma"),
        );
        let reader = std::io::Cursor::new(input.into_bytes());

        let mut conn = index.write.lock().expect("write connection poisoned");
        let tx = conn.transaction().expect("transaction");
        tx.execute_batch("DELETE FROM packages;").expect("delete");
        let mut pkg_stmt = tx.prepare(PKG_INSERT_SQL).expect("prepare pkg");

        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, reader).expect("index");

        drop(pkg_stmt);
        tx.commit().expect("commit");
        drop(conn);

        assert_eq!(aur_count, 3, "three valid rows should be indexed");
        assert_eq!(skipped, 1, "only the malformed object counts as skipped");

        let check = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("reopen");
        let names: Vec<String> = check
            .prepare("SELECT name FROM packages ORDER BY name")
            .expect("select names")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map")
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            names,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            "valid rows should land in packages",
        );

        let alpha_detail: String = check
            .query_row(
                "SELECT detail_json FROM packages WHERE name = 'alpha'",
                [],
                |row| row.get(0),
            )
            .expect("alpha detail_json");
        assert_eq!(alpha_detail, aur_json(1, "alpha"));
    }

    #[test]
    fn index_aur_rows_populates_keywords() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let row = r#"{"ID":7,"Name":"hex-tools","PackageBaseID":7,"PackageBase":"hex-tools","Version":"1.0-1","Description":"hex stuff","NumVotes":0,"Popularity":0.0,"FirstSubmitted":0,"LastModified":0,"Keywords":["hex","binary"]}"#;
        let input = format!("[\n{row}\n]");
        let reader = std::io::Cursor::new(input.into_bytes());

        let mut conn = index.write.lock().expect("write connection poisoned");
        let tx = conn.transaction().expect("transaction");
        tx.execute_batch("DELETE FROM packages;").expect("delete");
        let mut pkg_stmt = tx.prepare(PKG_INSERT_SQL).expect("prepare pkg");
        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, reader).expect("index");
        drop(pkg_stmt);
        tx.commit().expect("commit");
        drop(conn);

        assert_eq!(aur_count, 1, "one valid row should be indexed");
        assert_eq!(skipped, 0, "no malformed rows");

        let check = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("reopen");
        let keywords: String = check
            .query_row(
                "SELECT keywords FROM packages WHERE name = 'hex-tools'",
                [],
                |row| row.get(0),
            )
            .expect("keywords query");
        assert_eq!(
            keywords, "hex binary",
            "keywords should be space-joined into the column"
        );
    }

    #[test]
    fn fail_loud_boundary_at_one_percent() {
        assert!(!fail_loud(99, 1), "exactly 1% bad should pass the gate");
        assert!(fail_loud(98, 2), "above 1% bad should trip the gate");
        assert!(fail_loud(99, 2), "above 1% bad should trip the gate");
        assert!(!fail_loud(0, 0), "an empty dump should pass the gate");
    }

    #[test]
    fn gate_trip_rolls_back_and_records_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let input = format!(
            "[\n{}\nbroken-one\nbroken-two\nbroken-three\n]",
            aur_json(1, "good"),
        );
        let reader = std::io::Cursor::new(input.into_bytes());

        let mut conn = index.write.lock().expect("write connection poisoned");
        let tx = conn.transaction().expect("transaction");
        tx.execute_batch("DELETE FROM packages;").expect("delete");
        let mut pkg_stmt = tx.prepare(PKG_INSERT_SQL).expect("prepare pkg");

        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, reader).expect("index");
        let tripped = fail_loud(aur_count, skipped);
        let msg = format!(
            "too many malformed AUR rows: {skipped} of {}",
            aur_count + skipped
        );

        drop(pkg_stmt);
        drop(tx);
        meta_set(&conn, "last_error", &msg).expect("record last_error");
        drop(conn);

        assert!(tripped, "gate should trip when malformed rows exceed 1%");
        assert_eq!((aur_count, skipped), (1, 3));
        assert_eq!(msg, "too many malformed AUR rows: 3 of 4");

        let check = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("reopen");
        let pkg_count: i64 = check
            .query_row("SELECT COUNT(*) FROM packages", [], |row| row.get(0))
            .expect("count packages");
        assert_eq!(pkg_count, 0, "rolled-back transaction must leave no rows");
        let last_error: Option<String> = check
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_error'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(
            last_error.as_deref(),
            Some("too many malformed AUR rows: 3 of 4"),
            "meta.last_error should be recorded on gate trip",
        );
    }

    #[test]
    fn detail_correctly_queried_and_omitted_keys_are_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = LocalIndex::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("seed");

        let chrome = r#"{"ID":2154588,"Name":"google-chrome","PackageBaseID":37469,"PackageBase":"google-chrome","Version":"150.0.7871.114-1","Description":"The popular web browser by Google","URL":"https://www.google.com/chrome","NumVotes":2358,"Popularity":11.783191,"OutOfDate":null,"Maintainer":"gromit","Submitter":null,"FirstSubmitted":1274819156,"LastModified":1783555607,"URLPath":"/cgit/aur.git/snapshot/google-chrome.tar.gz","Depends":["alsa-lib","gtk3","libcups","libxss","libxtst","nss","ttf-liberation","xdg-utils"],"OptDepends":["pipewire","kdialog","gnome-keyring","kwallet"],"License":["custom:chrome"],"Keywords":["chromium"]}"#;
        let yay = r#"{"Depends":["pacman>6.1","git"],"Description":"Yet another yogurt.","FirstSubmitted":1475688004,"ID":2131240,"Keywords":["arm"],"LastModified":1781905288,"License":["GPL-3.0-or-later"],"Maintainer":"jguer","MakeDepends":["go>=1.24"],"Name":"yay","NumVotes":2617,"OptDepends":["sudo","doas"],"OutOfDate":null,"PackageBase":"yay","PackageBaseID":115973,"Popularity":40.475635,"Submitter":"jguer","URL":"https://github.com/Jguer/yay","URLPath":"/cgit/aur.git/snapshot/yay.tar.gz","Version":"13.0.1-1"}"#;
        let brave = r#"{"Conflicts":["brave"],"Depends":["alsa-lib","gtk3"],"Description":"Web browser","FirstSubmitted":1459948564,"ID":2163403,"Keywords":["brave"],"LastModified":1784135902,"License":["BSD"],"Maintainer":"brave","Name":"brave-bin","NumVotes":1021,"OptDepends":["cups"],"OutOfDate":null,"PackageBase":"brave-bin","PackageBaseID":109775,"Popularity":25.129281,"Provides":["brave=1.92.140","brave-browser"],"Submitter":"toropisco","URL":"https://www.brave.com","URLPath":"/cgit/aur.git/snapshot/brave-bin.tar.gz","Version":"1:1.92.140-1"}"#;

        for (name, blob) in [
            ("google-chrome", chrome),
            ("yay", yay),
            ("brave-bin", brave),
        ] {
            conn.execute(
                "INSERT INTO packages (name, detail_json) VALUES (?, ?)",
                rusqlite::params![name, blob],
            )
            .expect("seed");
        }
        conn.execute("INSERT INTO packages (name) VALUES ('repo-pkg')", [])
            .expect("seed null blob");
        drop(conn);

        let chrome_info = index
            .detail("google-chrome")
            .expect("chrome query")
            .expect("chrome present");
        assert_eq!(chrome_info.name, "google-chrome");
        assert_eq!(chrome_info.version, "150.0.7871.114-1");
        assert_eq!(chrome_info.depends.len(), 8);
        assert_eq!(chrome_info.opt_depends.len(), 4);
        assert_eq!(chrome_info.license, vec!["custom:chrome".to_string()]);
        assert_eq!(chrome_info.maintainer.as_deref(), Some("gromit"));
        assert!(
            chrome_info.make_depends.is_empty(),
            "omitted MakeDepends must deserialize empty"
        );
        assert!(
            chrome_info.provides.is_empty(),
            "omitted Provides must deserialize empty"
        );
        assert!(
            chrome_info.conflicts.is_empty(),
            "omitted Conflicts must deserialize empty"
        );

        let yay_info = index
            .detail("yay")
            .expect("yay query")
            .expect("yay present");
        assert_eq!(yay_info.make_depends, vec!["go>=1.24".to_string()]);
        assert!(yay_info.depends.contains(&"git".to_string()));

        let brave_info = index
            .detail("brave-bin")
            .expect("brave query")
            .expect("brave present");
        assert_eq!(brave_info.conflicts, vec!["brave".to_string()]);
        assert_eq!(
            brave_info.provides,
            vec!["brave=1.92.140".to_string(), "brave-browser".to_string()]
        );

        assert!(
            index.detail("missing").expect("missing query").is_none(),
            "unknown package must map to None"
        );
        assert!(
            index.detail("repo-pkg").expect("repo query").is_none(),
            "NULL detail_json must map to None"
        );
    }

    #[test]
    fn put_detail_writes_then_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let blob = r#"{"Depends":["pacman>6.1","git"],"Description":"Yet another yogurt.","FirstSubmitted":1475688004,"ID":2131240,"Keywords":["arm"],"LastModified":1781905288,"License":["GPL-3.0-or-later"],"Maintainer":"jguer","MakeDepends":["go>=1.24"],"Name":"yay","NumVotes":2617,"OptDepends":["sudo","doas"],"OutOfDate":null,"PackageBase":"yay","PackageBaseID":115973,"Popularity":40.475635,"Submitter":"jguer","URL":"https://github.com/Jguer/yay","URLPath":"/cgit/aur.git/snapshot/yay.tar.gz","Version":"13.0.1-1"}"#;
        let info: AurInfo = serde_json::from_str(blob).expect("parse yay");

        index.put_detail(&info).expect("first put_detail");
        let got = index
            .detail("yay")
            .expect("query")
            .expect("present after write");
        assert_eq!(got.name, "yay");
        assert_eq!(got.version, "13.0.1-1");
        assert_eq!(got.make_depends, vec!["go>=1.24".to_string()]);

        {
            let conn = index.read.lock().expect("read connection poisoned");
            let keywords: String = conn
                .query_row(
                    "SELECT keywords FROM packages WHERE name = 'yay'",
                    [],
                    |row| row.get(0),
                )
                .expect("keywords query");
            assert_eq!(
                keywords, "arm",
                "keywords should be space-joined after put_detail"
            );
        }

        let mut updated = info.clone();
        updated.version = "14.0.0-1".to_string();
        index.put_detail(&updated).expect("overwrite put_detail");
        let got2 = index
            .detail("yay")
            .expect("query")
            .expect("present after overwrite");
        assert_eq!(
            got2.version, "14.0.0-1",
            "put_detail must overwrite an existing row"
        );
        assert_eq!(got2.name, "yay");
        assert_eq!(got2.make_depends, vec!["go>=1.24".to_string()]);
    }

    #[test]
    fn search_recent_repo_prefix_filters_source_and_sorts_by_recency() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");
        let conn = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("seed");

        let insert = |name: &str, source: &str, last_update: i64| {
            conn.execute(
                "INSERT INTO packages (name, source, repo, version, description, num_votes, popularity, last_update, package_base) \
                 VALUES (?, ?, 'repo', '1', NULL, NULL, NULL, ?, NULL)",
                rusqlite::params![name, source, last_update],
            )
            .expect("seed");
        };

        insert("chromium", "repo", 1000);
        insert("curl", "repo", 500);
        insert("cmake", "repo", 200);
        insert("c-aur-pkg", "aur", 9999);
        insert("firefox", "repo", 9999);

        let rows = index.search_recent_repo_prefix("c", 50).expect("query");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["chromium", "curl", "cmake"]);
        assert!(rows.iter().all(|r| r.source == "repo"));
    }

    #[test]
    fn search_recent_repo_prefix_respects_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");
        let conn = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("seed");
        for i in 0..10 {
            let name = format!("c{i:02}");
            conn.execute(
                "INSERT INTO packages (name, source, repo, version, description, num_votes, popularity, last_update, package_base) \
                 VALUES (?, 'repo', 'repo', '1', NULL, NULL, NULL, ?, NULL)",
                rusqlite::params![name, i],
            )
            .expect("seed");
        }
        let rows = index.search_recent_repo_prefix("c", 5).expect("query");
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].name, "c09");
        assert_eq!(rows[4].name, "c05");
    }

    #[test]
    fn bm25_weights_name_above_description() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = LocalIndex::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("seed");

        conn.execute(
            "INSERT INTO packages \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base,detail_json,keywords) \
             VALUES ('gizmo','something else','aur','aur','1',0,0.0,0,NULL,NULL,NULL)",
            [],
        )
        .expect("seed gizmo");
        conn.execute(
            "INSERT INTO packages \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base,detail_json,keywords) \
             VALUES ('alpha','gizmo','aur','aur','1',0,0.0,0,NULL,NULL,NULL)",
            [],
        )
        .expect("seed alpha");
        drop(conn);

        let rows = index.search("gizmo*", 10).expect("search");
        assert_eq!(rows.len(), 2, "both rows match the gizmo* prefix token");
        assert_eq!(
            rows[0].name, "gizmo",
            "name match must outrank description-only match under bm25 weights"
        );
    }

    #[test]
    fn last_refreshed_age_is_none_when_unset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open");
        assert!(index.last_refreshed_age().is_none());
    }

    #[test]
    fn last_refreshed_age_is_none_for_garbage_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = LocalIndex::open(&path).expect("open");
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
