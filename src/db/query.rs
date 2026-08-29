pub struct PackageRow {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub repo: Option<String>,
    pub version: String,
    pub last_update: Option<i64>,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
    #[allow(dead_code)]
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

    pub fn names_with_prefix(&self, prefix: &str, limit: usize) -> anyhow::Result<Vec<String>> {
        let conn = self.read.lock().expect("read connection poisoned");
        match prefix_upper_bound(prefix) {
            Some(upper) => {
                let mut stmt = conn.prepare(
                    "SELECT name FROM packages WHERE name >= ?1 AND name < ?2 ORDER BY name LIMIT ?3",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![prefix, upper, limit as i64], |row| {
                        row.get::<_, String>(0)
                    })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = conn
                    .prepare("SELECT name FROM packages WHERE name >= ?1 ORDER BY name LIMIT ?2")?;
                let rows = stmt.query_map(rusqlite::params![prefix, limit as i64], |row| {
                    row.get::<_, String>(0)
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }
}

fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(&last) = chars.last() {
        let mut next = last as u32 + 1;
        if (0xD800..=0xDFFF).contains(&next) {
            next = 0xE000;
        }
        match char::from_u32(next) {
            Some(c) => {
                chars.pop();
                chars.push(c);
                return Some(chars.into_iter().collect());
            }
            None => {
                chars.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn prefix_upper_bound_increments_last_char() {
        assert_eq!(prefix_upper_bound("al"), Some("am".to_string()));
        assert_eq!(prefix_upper_bound("z"), Some("{".to_string()));
        assert_eq!(prefix_upper_bound(""), None);
        assert_eq!(
            prefix_upper_bound("a\u{10FFFF}"),
            Some("b".to_string())
        );
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

    #[test]
    fn names_with_prefix_filters_and_limits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let seed = rusqlite::Connection::open(&path).expect("seed");
        for name in ["alpha", "alpine", "al1", "al2", "al3", "al4", "a_b", "axb"] {
            seed.execute(
                "INSERT INTO packages (name) VALUES (?)",
                rusqlite::params![name],
            )
            .expect("insert");
        }
        drop(seed);

        assert_eq!(
            index.names_with_prefix("al", 3).expect("query"),
            vec!["al1".to_string(), "al2".to_string(), "al3".to_string()],
            "ORDER BY name with LIMIT truncates after three rows"
        );
        assert_eq!(
            index.names_with_prefix("al", 10).expect("query"),
            vec![
                "al1".to_string(),
                "al2".to_string(),
                "al3".to_string(),
                "al4".to_string(),
                "alpha".to_string(),
                "alpine".to_string(),
            ]
        );
        assert_eq!(
            index.names_with_prefix("a_", 10).expect("query"),
            vec!["a_b".to_string()],
            "underscore is matched literally via range bounds"
        );
        assert_eq!(
            index.names_with_prefix("ax", 10).expect("query"),
            vec!["axb".to_string()]
        );
        assert!(
            index.names_with_prefix("zz", 10).expect("query").is_empty(),
            "no matches yields an empty list"
        );
    }
}
