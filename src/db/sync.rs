use super::fetch::{AUR_META_URL, DecompressedDump, FetchOutcome, fetch};
use super::{join_list, meta_get, meta_set};

#[derive(Debug)]
pub enum RefreshOutcome {
    NotModified,
    Updated {
        aur_count: usize,
        repo_count: usize,
        skipped: usize,
    },
}

impl super::PackageDb {
    pub fn refresh(&self, handle: &alpm::Alpm) -> anyhow::Result<RefreshOutcome> {
        let last_modified = {
            let conn = self.write.lock().expect("write connection poisoned");
            meta_get(&conn, "last_modified")?
        };
        let mut dump = match fetch(AUR_META_URL, last_modified.as_deref())? {
            FetchOutcome::NotModified => return Ok(RefreshOutcome::NotModified),
            FetchOutcome::Updated(dump) => dump,
        };

        let mut conn = self.write.lock().expect("write connection poisoned");
        self.sync_from_dump(&mut conn, handle, &mut dump)
    }
}

impl super::PackageDb {
    fn sync_from_dump(
        &self,
        conn: &mut rusqlite::Connection,
        handle: &alpm::Alpm,
        dump: &mut DecompressedDump,
    ) -> anyhow::Result<RefreshOutcome> {
        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM packages;")?;

        let mut pkg_stmt = tx.prepare(&super::pkg_insert_sql())?;

        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, dump.reader())?;

        if fail_loud(aur_count, skipped) {
            let total = aur_count + skipped;
            let msg = format!("too many malformed AUR rows: {skipped} of {total}");
            drop(pkg_stmt);
            drop(tx);
            let _ = meta_set(conn, "last_error", &msg);
            return Err(anyhow::Error::msg(msg));
        }

        let null_votes: Option<i64> = None;
        let null_popularity: Option<f64> = None;
        let null_base: Option<&str> = None;
        let null_keywords: Option<&str> = None;
        let null_url: Option<&str> = None;
        let null_out_of_date: Option<i64> = None;
        let null_maintainer: Option<&str> = None;
        let null_license: Option<&str> = None;
        let null_depends: Option<&str> = None;
        let null_make_depends: Option<&str> = None;
        let null_check_depends: Option<&str> = None;
        let null_opt_depends: Option<&str> = None;
        let null_conflicts: Option<&str> = None;
        let null_provides: Option<&str> = None;
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
                    &null_url,
                    &null_out_of_date,
                    &null_maintainer,
                    &null_license,
                    &null_depends,
                    &null_make_depends,
                    &null_check_depends,
                    &null_opt_depends,
                    &null_conflicts,
                    &null_provides,
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
        meta_set(conn, "last_modified", dump.last_modified())?;
        meta_set(conn, "last_refreshed", &now.to_string())?;
        meta_set(conn, "aur_count", &aur_count.to_string())?;
        meta_set(conn, "repo_count", &repo_count.to_string())?;
        meta_set(conn, "last_error", "")?;

        Ok(RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        })
    }
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
                let depends = join_list(&info.depends);
                let make_depends = join_list(&info.make_depends);
                let check_depends = join_list(&info.check_depends);
                let opt_depends = join_list(&info.opt_depends);
                let conflicts = join_list(&info.conflicts);
                let provides = join_list(&info.provides);
                let license = join_list(&info.license);
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
                    &info.url,
                    info.out_of_date,
                    &info.maintainer,
                    &license,
                    &depends,
                    &make_depends,
                    &check_depends,
                    &opt_depends,
                    &conflicts,
                    &provides,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PackageDb;
    use crate::db::fetch::DecompressedDump;
    use crate::db::pkg_insert_sql;
    use alpm::Alpm;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn aur_json(id: u64, name: &str) -> String {
        format!(
            r#"{{"ID":{id},"Name":"{name}","PackageBaseID":{id},"PackageBase":"{name}","Version":"1.0-1","NumVotes":0,"Popularity":0.0,"FirstSubmitted":0,"LastModified":0,"URL":"https://example.com/{name}","Maintainer":"maint-{name}","Depends":["a","b"],"MakeDepends":["c"],"OptDepends":["d: thing"]}}"#
        )
    }

    #[test]
    fn index_aur_rows_maps_all_detail_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let row = r#"{"ID":1,"Name":"allinone","PackageBaseID":1,"PackageBase":"allinone","Version":"1.0-1","Description":"everything package","NumVotes":7,"Popularity":3.5,"FirstSubmitted":100,"LastModified":1700000000,"URL":"https://example.com/allinone","OutOfDate":1234567890,"Maintainer":"mymaint","Depends":["a","b","c"],"MakeDepends":["m1","m2"],"CheckDepends":["c1"],"OptDepends":["foo: bar baz"],"Conflicts":["conf1"],"Provides":["prov1","prov2"],"License":["MIT","Apache-2.0"],"Keywords":["kw1","kw2"]}"#;
        let input = format!("[\n{row}\n]");
        let reader = std::io::Cursor::new(input.into_bytes());

        let mut conn = index.write.lock().expect("write connection poisoned");
        let tx = conn.transaction().expect("transaction");
        tx.execute_batch("DELETE FROM packages;").expect("delete");
        let mut pkg_stmt = tx.prepare(&pkg_insert_sql()).expect("prepare pkg");
        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, reader).expect("index");
        drop(pkg_stmt);
        tx.commit().expect("commit");
        drop(conn);

        assert_eq!(aur_count, 1, "one valid row should be indexed");
        assert_eq!(skipped, 0, "no malformed rows");

        let check = rusqlite::Connection::open(&path).expect("reopen");
        let text = |col: &str| -> String {
            check
                .query_row(
                    &format!("SELECT {col} FROM packages WHERE name = 'allinone'"),
                    [],
                    |r| r.get::<_, String>(0),
                )
                .expect("read text column")
        };
        let opt_text = |col: &str| -> Option<String> {
            check
                .query_row(
                    &format!("SELECT {col} FROM packages WHERE name = 'allinone'"),
                    [],
                    |r| r.get::<_, Option<String>>(0),
                )
                .expect("read opt text column")
        };
        let num = |col: &str| -> i64 {
            check
                .query_row(
                    &format!("SELECT {col} FROM packages WHERE name = 'allinone'"),
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .expect("read numeric column")
        };
        let opt_num = |col: &str| -> Option<i64> {
            check
                .query_row(
                    &format!("SELECT {col} FROM packages WHERE name = 'allinone'"),
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .expect("read opt numeric column")
        };
        let float = |col: &str| -> f64 {
            check
                .query_row(
                    &format!("SELECT {col} FROM packages WHERE name = 'allinone'"),
                    [],
                    |r| r.get::<_, f64>(0),
                )
                .expect("read float column")
        };

        assert_eq!(text("name"), "allinone", "name");
        assert_eq!(text("source"), "aur", "source");
        assert_eq!(text("repo"), "aur", "repo");
        assert_eq!(text("version"), "1.0-1", "version");
        assert_eq!(text("description"), "everything package", "description");
        assert_eq!(num("num_votes"), 7, "num_votes");
        assert_eq!(float("popularity"), 3.5, "popularity");
        assert_eq!(num("last_update"), 1700000000, "last_update");
        assert_eq!(text("package_base"), "allinone", "package_base");
        assert_eq!(text("url"), "https://example.com/allinone", "url");
        assert_eq!(opt_num("out_of_date"), Some(1234567890), "out_of_date");
        assert_eq!(opt_text("maintainer"), Some("mymaint".to_string()), "maintainer");
        assert_eq!(text("license"), "MIT\nApache-2.0", "license");
        assert_eq!(text("depends"), "a\nb\nc", "depends");
        assert_eq!(text("make_depends"), "m1\nm2", "make_depends");
        assert_eq!(text("check_depends"), "c1", "check_depends");
        assert_eq!(text("opt_depends"), "foo: bar baz", "opt_depends");
        assert_eq!(text("conflicts"), "conf1", "conflicts");
        assert_eq!(text("provides"), "prov1\nprov2", "provides");
        assert_eq!(text("keywords"), "kw1 kw2", "keywords");
    }

    #[test]
    fn index_aur_rows_parses_stream_and_skips_malformed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = PackageDb::open(&dir.path().join("aur-meta.sqlite")).expect("open");

        let input = format!(
            "[\n\
             \n\
             [{},\n\
             {}]\n\
             {{ broken object }}\n\
             ]",
            aur_json(1, "alpha"),
            aur_json(2, "beta"),
        );
        let reader = std::io::Cursor::new(input.into_bytes());

        let mut conn = index.write.lock().expect("write connection poisoned");
        let tx = conn.transaction().expect("transaction");
        tx.execute_batch("DELETE FROM packages;").expect("delete");
        let mut pkg_stmt = tx.prepare(&pkg_insert_sql()).expect("prepare pkg");
        let (aur_count, skipped) = index_aur_rows(&mut pkg_stmt, reader).expect("index");
        drop(pkg_stmt);
        tx.commit().expect("commit");
        drop(conn);

        assert_eq!(aur_count, 2, "two valid rows should be indexed");
        assert_eq!(skipped, 1, "the malformed object counts as skipped");

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
            vec!["alpha".to_string(), "beta".to_string()],
            "only valid rows should land in packages"
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
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let stream = format!(
            "[\n{}\nbroken-one\nbroken-two\nbroken-three\n]",
            aur_json(1, "good"),
        );
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream.as_bytes()).expect("gzip write");
        let gz = encoder.finish().expect("gzip finish");
        let reader: Box<dyn std::io::BufRead + Send> = Box::new(std::io::BufReader::new(
            GzDecoder::new(std::io::Cursor::new(gz)),
        ));
        let mut dump = DecompressedDump::from_reader(reader, "test-last-modified");

        let db_path = dir.path().join("alpmdb");
        std::fs::create_dir_all(db_path.join("local")).expect("local db dir");
        let handle = Alpm::new("/", db_path.to_str().expect("db path str")).expect("alpm handle");

        let mut conn = index.write.lock().expect("write connection poisoned");
        let result = index.sync_from_dump(&mut conn, &handle, &mut dump);
        drop(conn);

        let msg = "too many malformed AUR rows: 3 of 4";
        let err = result.expect_err("gate must return Err");
        assert_eq!(err.to_string(), msg, "error message must match the gate format");

        let check = rusqlite::Connection::open(&path).expect("reopen");
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
            Some(msg),
            "meta.last_error should be recorded on gate trip"
        );
    }

    #[test]
    fn sync_from_dump_indexes_aur_and_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let stream = format!("[\n{}\n{}\n]", aur_json(1, "aa"), aur_json(2, "bb"));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(stream.as_bytes()).expect("gzip write");
        let gz = encoder.finish().expect("gzip finish");
        let reader: Box<dyn std::io::BufRead + Send> = Box::new(std::io::BufReader::new(
            GzDecoder::new(std::io::Cursor::new(gz)),
        ));
        let mut dump = DecompressedDump::from_reader(reader, "test-last-modified");

        let db_path = dir.path().join("alpmdb");
        std::fs::create_dir_all(db_path.join("local")).expect("local db dir");
        let handle = Alpm::new("/", db_path.to_str().expect("db path str")).expect("alpm handle");

        let mut conn = index.write.lock().expect("write connection poisoned");
        let outcome = index
            .sync_from_dump(&mut conn, &handle, &mut dump)
            .expect("sync_from_dump");
        drop(conn);

        match outcome {
            RefreshOutcome::Updated {
                aur_count,
                repo_count,
                skipped,
            } => {
                assert_eq!(aur_count, 2, "two aur rows indexed");
                assert_eq!(repo_count, 0, "no sync dbs means zero repo rows");
                assert_eq!(skipped, 0, "no malformed rows");
            }
            RefreshOutcome::NotModified => panic!("sync_from_dump must return Updated"),
        }

        let check = rusqlite::Connection::open(&path).expect("reopen");
        let pkg_count: i64 = check
            .query_row("SELECT COUNT(*) FROM packages", [], |row| row.get(0))
            .expect("count packages");
        assert_eq!(pkg_count, 2, "two rows persisted");
        let names: Vec<String> = check
            .prepare("SELECT name FROM packages ORDER BY name")
            .expect("select names")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map")
            .map(Result::unwrap)
            .collect();
        assert_eq!(names, vec!["aa".to_string(), "bb".to_string()]);

        assert_eq!(
            index.get_meta("last_modified").expect("last_modified"),
            Some("test-last-modified".to_string())
        );
        assert!(
            index.get_meta("last_refreshed").expect("last_refreshed").is_some(),
            "last_refreshed must be recorded"
        );
        assert_eq!(
            index.get_meta("aur_count").expect("aur_count"),
            Some("2".to_string())
        );
        assert_eq!(
            index.get_meta("repo_count").expect("repo_count"),
            Some("0".to_string())
        );
        assert_eq!(
            index.get_meta("last_error").expect("last_error"),
            Some("".to_string()),
            "last_error must be cleared after a successful sync"
        );
    }
}
