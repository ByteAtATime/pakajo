#[cfg(test)]
mod search_engine_tests {
    use crate::db::PackageDb;
    use crate::search::engine::SearchEngine;
    use crate::search::index::index_path;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::UNIX_EPOCH;

    fn seed_package(conn: &rusqlite::Connection, name: &str, source: &str) {
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![name, source, source, "1.0-1", "", 0i64, 0.0f64, 0i64, name],
        )
        .expect("seed package");
    }

    fn rowid_to_name(sqlite_path: &Path) -> HashMap<u32, String> {
        let conn = rusqlite::Connection::open(sqlite_path).expect("open read conn");
        let mut stmt = conn
            .prepare("SELECT rowid, name FROM packages")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                Ok((id as u32, name))
            })
            .expect("query");
        rows.filter_map(Result::ok).collect()
    }

    #[test]
    fn search_engine_returns_ranked_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");
        seed_package(&conn, "chromium", "repo");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");

        let ids = engine.search("chrome");
        assert!(!ids.is_empty(), "chrome query must return results");

        let names = rowid_to_name(&sqlite_path);
        let top_name = ids
            .first()
            .and_then(|id| names.get(id))
            .expect("top id maps to a seeded package");
        assert_eq!(top_name, "google-chrome");
    }

    #[test]
    fn ensure_fresh_is_noop_when_index_fresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");
        let before = engine.search("chrome");

        engine.ensure_fresh().expect("ensure_fresh on fresh index");

        let after = engine.search("chrome");
        assert_eq!(before, after, "fresh index must not be rebuilt");
    }

    #[test]
    fn ensure_fresh_rebuilds_when_index_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");
        assert!(
            engine.search("firefox").is_empty(),
            "firefox absent before rebuild"
        );

        seed_package(&conn, "firefox", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");

        let index_bin = index_path(&sqlite_path);
        std::fs::File::open(&index_bin)
            .and_then(|f| f.set_modified(UNIX_EPOCH))
            .expect("backdate index.bin");

        engine.ensure_fresh().expect("ensure_fresh after stale");

        let names = rowid_to_name(&sqlite_path);
        let ids = engine.search("firefox");
        assert!(!ids.is_empty(), "firefox must appear after rebuild");
        let top = ids
            .first()
            .and_then(|id| names.get(id))
            .expect("top id maps to a seeded package");
        assert_eq!(top, "firefox");
    }
}
