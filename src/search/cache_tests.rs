use crate::db::{PackageDb, db_cache_fingerprint};
use crate::search::{cache_path, engine::SearchEngine, engine_for, hydrate_metas};
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

fn result_names(db: &PackageDb, engine: &SearchEngine, query: &str) -> Vec<String> {
    engine
        .query(query)
        .execute(|ids| hydrate_metas(db, ids))
        .expect("execute")
        .into_iter()
        .map(|result| result.name)
        .collect()
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

    let names = result_names(&db, &engine, "chrome");
    assert!(!names.is_empty(), "chrome query must return results");
    assert_eq!(names.into_iter().next().expect("top hit"), "google-chrome");
}

#[test]
fn rebuild_is_noop_when_fingerprint_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite_path = dir.path().join("aur-meta.sqlite");
    let db = PackageDb::open(&sqlite_path).expect("open");
    let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
    seed_package(&conn, "google-chrome", "aur");

    let engine = engine_for(&db, &cache_path(&sqlite_path)).expect("engine");
    let before = result_names(&db, &engine, "chrome");

    engine
        .rebuild(db_cache_fingerprint(&db), || db.index_rows())
        .expect("rebuild on unchanged db");

    let after = result_names(&db, &engine, "chrome");
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
        result_names(&db, &engine, "firefox").is_empty(),
        "firefox absent before rebuild"
    );

    seed_package(&conn, "firefox", "aur");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .expect("checkpoint");

    engine
        .rebuild(db_cache_fingerprint(&db), || db.index_rows())
        .expect("rebuild after insert");

    let names = result_names(&db, &engine, "firefox");
    assert!(!names.is_empty(), "firefox must appear after rebuild");
    assert_eq!(names.into_iter().next().expect("top hit"), "firefox");
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
    let names = result_names(&db, &engine, "firefox");
    assert!(!names.is_empty(), "recovered engine must serve rows");
    assert_eq!(names.into_iter().next().expect("top hit"), "firefox");
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
fn index_rows_matches_direct_select() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite_path = dir.path().join("aur-meta.sqlite");
    let db = PackageDb::open(&sqlite_path).expect("open");
    let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
    for (name, source, popularity, keywords) in [
        ("alpha", "aur", Some(12.5f64), Some("editor terminal")),
        ("beta", "repo", None, None),
        ("gamma", "aur", Some(0.0f64), Some("shell")),
    ] {
        conn.execute(
            "INSERT INTO packages (name,source,repo,version,popularity,keywords,package_base) \
             VALUES (?,?,?,?,?,?,?)",
            rusqlite::params![name, source, source, "1.0-1", popularity, keywords, name],
        )
        .expect("seed package");
    }

    let mut got = db.index_rows().expect("index rows");
    got.sort_by_key(|row| row.id);

    let read = rusqlite::Connection::open(&sqlite_path).expect("read conn");
    let mut stmt = read
        .prepare("SELECT rowid AS id, name, source, popularity, keywords FROM packages ORDER BY id")
        .expect("prepare");
    let expected: Vec<pakajo_search::IndexRow> = stmt
        .query_map([], |row| {
            Ok(pakajo_search::IndexRow {
                id: row.get::<_, i64>(0)? as u32,
                name: row.get(1)?,
                source: row.get(2)?,
                popularity: row.get(3)?,
                keywords: row.get(4)?,
            })
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");

    assert_eq!(got.len(), 3, "three seeded packages must surface");
    assert_eq!(got.len(), expected.len(), "row counts must agree");
    for (left, right) in got.iter().zip(expected.iter()) {
        assert_eq!(left.id, right.id);
        assert_eq!(left.name, right.name);
        assert_eq!(left.source, right.source);
        assert_eq!(left.popularity, right.popularity);
        assert_eq!(left.keywords, right.keywords);
    }
}

#[test]
fn engine_for_fails_loud_when_db_file_moved_away() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite_path = dir.path().join("aur-meta.sqlite");
    let db = PackageDb::open(&sqlite_path).expect("open");
    let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
    seed_package(&conn, "firefox", "aur");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .expect("checkpoint");

    let cache = cache_path(&sqlite_path);
    std::fs::rename(&sqlite_path, dir.path().join("aur-meta.sqlite.moved")).expect("move db away");
    conn.execute_batch("DROP TABLE packages").expect("drop");

    match engine_for(&db, &cache) {
        Ok(_) => panic!("moved-away db must fail loudly, never yield an empty engine"),
        Err(err) => assert!(
            matches!(err, pakajo_search::SearchError::Store(_)),
            "moved-away db must surface a Store error, got {err:?}"
        ),
    }
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
