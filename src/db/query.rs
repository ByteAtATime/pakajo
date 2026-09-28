pub struct PackageRow {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub repo: Option<String>,
    pub version: String,
    pub last_update: Option<i64>,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
    pub package_base: Option<String>,
}

fn row_to_package(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRow> {
    Ok(PackageRow {
        name: row.get(0)?,
        description: row.get(1)?,
        source: row.get(2)?,
        repo: row.get(3)?,
        version: row.get(4)?,
        last_update: row.get(5)?,
        package_base: row.get(6)?,
        num_votes: row.get(7)?,
        popularity: row.get(8)?,
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

    pub fn hydrate_by_ids(
        &self,
        ids: &[u32],
    ) -> anyhow::Result<std::collections::HashMap<u32, PackageRow>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT name, description, source, repo, version, \
             last_update, package_base, num_votes, popularity, rowid \
             FROM packages WHERE rowid IN ({placeholders})"
        );
        let conn = self.read.lock().expect("read connection poisoned");
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let pkg = row_to_package(row)?;
            let rowid: i64 = row.get(9)?;
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
