use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pakajo_search::engine::SearchEngine;
use pakajo_search::{SearchError, SearchFilter, SearchResult, Source};

use super::PackageDb;

struct PackageRow {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub repo: Option<String>,
    pub version: String,
    pub last_update: Option<i64>,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
}

pub struct SearchSession {
    engine: Arc<SearchEngine>,
    db: Arc<PackageDb>,
}

impl SearchSession {
    pub fn open(db: Arc<PackageDb>) -> Result<Self, SearchError> {
        let cache = db.index_cache_path();
        let fingerprint = super::db_cache_fingerprint(&db);
        let build =
            || SearchEngine::build(Some((cache.as_path(), fingerprint)), || db.index_rows());
        let engine = match build() {
            Err(SearchError::Corrupt) => {
                eprintln!("search cache corrupt, rebuilding");
                let _ = std::fs::remove_file(&cache);
                build()?
            }
            other => other?,
        };
        Ok(Self {
            engine: Arc::new(engine),
            db,
        })
    }

    pub fn query(
        &self,
        text: &str,
        filter: SearchFilter,
        installed: &HashSet<String>,
        groups: &[(String, String)],
    ) -> Result<Vec<SearchResult>, SearchError> {
        self.engine
            .query(text)
            .filter(filter)
            .installed(installed)
            .groups(groups)
            .execute(|ids| hydrate_results(&self.db, ids))
    }

    pub fn complete_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.engine.complete_prefix(prefix, limit)
    }

    pub fn is_empty(&self) -> bool {
        self.engine.is_empty()
    }

    pub fn rebuild(&self) -> Result<(), SearchError> {
        self.engine
            .rebuild(super::db_cache_fingerprint(&self.db), || {
                self.db.index_rows()
            })
    }
}

pub fn hydrate_results(
    db: &PackageDb,
    ids: &[u32],
) -> Result<HashMap<u32, SearchResult>, SearchError> {
    let rows = match db.hydrate_by_ids(ids) {
        Ok(rows) => rows,
        Err(e) => match e.downcast::<rusqlite::Error>() {
            Ok(store) => return Err(SearchError::Store(Box::new(store))),
            Err(e) => return Err(SearchError::Store(e.into_boxed_dyn_error())),
        },
    };
    let mut metas = HashMap::with_capacity(rows.len());
    for &id in ids {
        match rows.get(&id) {
            Some(row) => {
                metas.insert(id, row_to_result(row));
            }
            None => {
                eprintln!("warning: search result id {id} missing from package database, skipping");
            }
        }
    }
    Ok(metas)
}

pub fn apply_installed_to_results(results: &mut [SearchResult], installed: &HashSet<String>) {
    for result in results.iter_mut() {
        result.installed = installed.contains(&result.name);
    }
}

fn row_to_result(row: &PackageRow) -> SearchResult {
    SearchResult {
        name: row.name.clone(),
        description: row
            .description
            .clone()
            .map(|d| crate::color::strip_controls(&d).into_owned()),
        source: if row.source == "repo" {
            Source::Repo
        } else {
            Source::Aur
        },
        repo: row.repo.clone(),
        version: Some(row.version.clone()),
        installed: false,
        last_update: row.last_update.unwrap_or(0),
        num_votes: row.num_votes.unwrap_or(0),
        popularity: row.popularity.unwrap_or(0.0),
    }
}

fn row_to_package(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRow> {
    Ok(PackageRow {
        name: row.get(0)?,
        description: row.get(1)?,
        source: row.get(2)?,
        repo: row.get(3)?,
        version: row.get(4)?,
        last_update: row.get(5)?,
        num_votes: row.get(6)?,
        popularity: row.get(7)?,
    })
}

impl super::PackageDb {
    pub fn index_rows(&self) -> Result<Vec<pakajo_search::IndexRow>, pakajo_search::SearchError> {
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn
            .prepare("SELECT rowid AS id, name, source, popularity, keywords FROM packages")
            .map_err(store_error)?;
        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                Ok(pakajo_search::IndexRow {
                    id: id as u32,
                    name: row.get(1)?,
                    source: row.get(2)?,
                    popularity: row.get(3)?,
                    keywords: row.get(4)?,
                })
            })
            .map_err(store_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(store_error)
    }

    fn hydrate_by_ids(
        &self,
        ids: &[u32],
    ) -> anyhow::Result<std::collections::HashMap<u32, PackageRow>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT name, description, source, repo, version, last_update, \
             num_votes, popularity, rowid FROM packages WHERE rowid IN ({placeholders})"
        );
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let pkg = row_to_package(row)?;
            let rowid: i64 = row.get(8)?;
            Ok((rowid as u32, pkg))
        })?;
        rows.collect::<rusqlite::Result<std::collections::HashMap<u32, PackageRow>>>()
            .map_err(anyhow::Error::from)
    }
}

fn store_error(e: rusqlite::Error) -> pakajo_search::SearchError {
    pakajo_search::SearchError::Store(Box::new(e))
}

#[cfg(test)]
mod tests {
    use crate::db::PackageDb;

    #[test]
    fn hydrate_by_ids_returns_rows_keyed_by_rowid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let empty = index.hydrate_by_ids(&[]).expect("hydrate empty");
        assert!(empty.is_empty(), "empty input yields an empty map");

        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute(
            "INSERT INTO packages \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base) \
             VALUES ('alpha','desc a','aur','aur','1.0-1',10,1.5,100,'alpha')",
            [],
        )
        .expect("seed alpha");
        conn.execute(
            "INSERT INTO packages \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base) \
             VALUES ('beta','desc b','repo','core','2.0-1',NULL,NULL,200,'beta')",
            [],
        )
        .expect("seed beta");
        drop(conn);

        let rows = index.hydrate_by_ids(&[1, 2, 99]).expect("hydrate");
        assert_eq!(rows.len(), 2, "known rowids returned, unknown omitted");
        assert!(rows.contains_key(&1), "rowid 1 present");
        assert!(rows.contains_key(&2), "rowid 2 present");
        assert!(!rows.contains_key(&99), "unknown rowid 99 must be absent");
        let alpha = rows.get(&1).expect("rowid 1 present");
        assert_eq!(alpha.name, "alpha");
        assert_eq!(alpha.source, "aur");
        let beta = rows.get(&2).expect("rowid 2 present");
        assert_eq!(beta.name, "beta");
        assert_eq!(beta.source, "repo");
    }

    #[test]
    fn packages_name_column_is_primary_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let _index = PackageDb::open(&path).expect("open");

        let conn = rusqlite::Connection::open(&path).expect("raw conn");
        let rows = conn
            .prepare("PRAGMA table_info(packages)")
            .expect("prepare pragma")
            .query_map([], |row| {
                let col: String = row.get(1)?;
                let pk_flag: i64 = row.get(5)?;
                Ok((col, pk_flag))
            })
            .expect("query pragma")
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        drop(conn);

        let pk = rows
            .iter()
            .find(|(col, _)| col == "name")
            .map(|(_, flag)| *flag)
            .expect("name column present");
        assert!(
            pk > 0,
            "name must be the primary key so prefix range scans hit the PK index"
        );
    }
}
