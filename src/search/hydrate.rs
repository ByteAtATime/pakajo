use std::collections::{HashMap, HashSet};

use crate::db::{PackageDb, PackageRow};

pub fn apply_installed_to_results(
    results: &mut [pakajo_search::SearchResult],
    installed: &HashSet<String>,
) {
    for result in results.iter_mut() {
        result.installed = installed.contains(&result.name);
    }
}

pub fn hydrate_metas(
    db: &PackageDb,
    ids: &[u32],
) -> Result<HashMap<u32, pakajo_search::PackageMeta>, pakajo_search::SearchError> {
    let rows = match db.hydrate_by_ids(ids) {
        Ok(rows) => rows,
        Err(e) => match e.downcast::<rusqlite::Error>() {
            Ok(store) => return Err(pakajo_search::SearchError::Store(Box::new(store))),
            Err(e) => return Err(pakajo_search::SearchError::Store(e.into_boxed_dyn_error())),
        },
    };
    let mut metas = HashMap::with_capacity(rows.len());
    for &id in ids {
        match rows.get(&id) {
            Some(row) => {
                metas.insert(id, row_to_meta(row));
            }
            None => {
                eprintln!("warning: search result id {id} missing from package database, skipping");
            }
        }
    }
    Ok(metas)
}

fn row_to_meta(row: &PackageRow) -> pakajo_search::PackageMeta {
    let source = match row.source.as_str() {
        "aur" => pakajo_search::Source::Aur,
        _ => pakajo_search::Source::Repo,
    };
    pakajo_search::PackageMeta {
        name: row.name.clone(),
        description: row
            .description
            .clone()
            .map(|d| crate::color::strip_controls(&d).into_owned()),
        source,
        repo: row.repo.clone(),
        version: Some(row.version.clone()),
        last_update: row.last_update.unwrap_or(0),
        num_votes: row.num_votes.unwrap_or(0),
        popularity: row.popularity.unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_installed_to_results_flips_only_members() {
        fn crate_result(name: &str) -> pakajo_search::SearchResult {
            pakajo_search::SearchResult {
                name: name.to_string(),
                source: pakajo_search::Source::Aur,
                description: Some("desc".to_string()),
                version: Some("1.0-1".to_string()),
                repo: Some("core".to_string()),
                installed: false,
                num_votes: 0,
                popularity: 0.0,
                last_update: 100,
            }
        }
        let mut results = vec![crate_result("vim"), crate_result("emacs")];
        let mut installed = HashSet::new();
        installed.insert("vim".to_string());
        apply_installed_to_results(&mut results, &installed);
        assert!(results[0].installed);
        assert!(!results[1].installed);
    }
}
