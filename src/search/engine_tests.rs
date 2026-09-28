#[cfg(test)]
mod search_engine_tests {
    use crate::db::{PackageDb, db_cache_fingerprint};
    use crate::search::{SearchFilter, cache_path, engine::SearchEngine, engine_for};
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    fn seed_package(conn: &rusqlite::Connection, name: &str, source: &str) {
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![name, source, source, "1.0-1", "", 0i64, 0.0f64, 0i64, name],
        )
        .expect("seed package");
    }

    fn result_ids(engine: &SearchEngine, query: &str) -> Vec<u32> {
        engine
            .search_tiered(query, SearchFilter::All, &HashSet::new())
            .into_iter()
            .map(|(id, _)| id)
            .collect()
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

    fn top_name(sqlite_path: &Path, ids: &[u32]) -> String {
        let names = rowid_to_name(sqlite_path);
        ids.first()
            .and_then(|id| names.get(id))
            .expect("top id maps to a seeded package")
            .clone()
    }

    #[test]
    fn engine_for_returns_ranked_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");
        seed_package(&conn, "chromium", "repo");

        let engine = engine_for(&db, &cache_path(&sqlite_path)).expect("engine");

        let ids = result_ids(&engine, "chrome");
        assert!(!ids.is_empty(), "chrome query must return results");
        assert_eq!(top_name(&sqlite_path, &ids), "google-chrome");
    }

    #[test]
    fn rebuild_is_noop_when_fingerprint_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = engine_for(&db, &cache_path(&sqlite_path)).expect("engine");
        let before = result_ids(&engine, "chrome");

        engine
            .rebuild(db_cache_fingerprint(&db), || db.index_rows())
            .expect("rebuild on unchanged db");

        let after = result_ids(&engine, "chrome");
        assert_eq!(before, after, "unchanged db must not be rebuilt");
    }

    #[test]
    fn rebuild_picks_up_new_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = engine_for(&db, &cache_path(&sqlite_path)).expect("engine");
        assert!(
            result_ids(&engine, "firefox").is_empty(),
            "firefox absent before rebuild"
        );

        seed_package(&conn, "firefox", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");

        engine
            .rebuild(db_cache_fingerprint(&db), || db.index_rows())
            .expect("rebuild after insert");

        let ids = result_ids(&engine, "firefox");
        assert!(!ids.is_empty(), "firefox must appear after rebuild");
        assert_eq!(top_name(&sqlite_path, &ids), "firefox");
    }

    #[test]
    fn corrupt_cache_is_rebuilt_from_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "firefox", "aur");

        let cache = cache_path(&sqlite_path);
        engine_for(&db, &cache).expect("initial build");
        std::fs::write(&cache, b"not an index").expect("corrupt cache");

        let engine = engine_for(&db, &cache).expect("recovery build");
        let ids = result_ids(&engine, "firefox");
        assert!(!ids.is_empty(), "recovered engine must serve rows");
        assert_eq!(top_name(&sqlite_path, &ids), "firefox");
    }

    #[test]
    fn provider_failure_surfaces_as_store_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "firefox", "aur");

        let cache = cache_path(&sqlite_path);
        let engine = engine_for(&db, &cache).expect("engine");

        conn.execute_batch("DROP TABLE packages").expect("drop");

        let build_err = match engine_for(&db, &cache) {
            Ok(_) => panic!("dropped table must fail the build"),
            Err(err) => err,
        };
        assert!(
            matches!(build_err, pakajo_search::SearchError::Store(_)),
            "provider failure must surface as a Store error"
        );
        let rebuild_err = match engine.rebuild(db_cache_fingerprint(&db), || db.index_rows()) {
            Ok(()) => panic!("dropped table must fail the rebuild"),
            Err(err) => err,
        };
        assert!(
            matches!(rebuild_err, pakajo_search::SearchError::Store(_)),
            "provider failure must surface as a Store error"
        );
    }

    #[test]
    fn fingerprint_changes_on_size_only_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        let pinned = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        seed_package(&conn, "alpha", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");
        pin_mtime(&sqlite_path, pinned);
        let before = db_cache_fingerprint(&db);

        seed_package(&conn, "beta", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");
        pin_mtime(&sqlite_path, pinned);
        let after = db_cache_fingerprint(&db);

        assert_ne!(
            before, after,
            "same mtime with different content must fingerprint differently"
        );
    }

    #[test]
    fn fingerprint_sees_uncheckpointed_wal_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let db = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");

        seed_package(&conn, "alpha", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");
        let settled = db_cache_fingerprint(&db);

        seed_package(&conn, "beta", "aur");
        let pending = db_cache_fingerprint(&db);

        assert_ne!(
            settled, pending,
            "a write sitting in -wal must change the fingerprint"
        );
    }

    fn pin_mtime(path: &Path, mtime: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open db file")
            .set_modified(mtime)
            .expect("pin mtime");
    }
}
