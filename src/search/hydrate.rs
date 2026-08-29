use std::collections::HashSet;

#[cfg(test)]
use std::collections::HashMap;

use crate::db::PackageRow;
use crate::package::PackageSource;

use super::SearchResult;

#[cfg(test)]
pub fn to_search_results(
    rows: &HashMap<u32, PackageRow>,
    ids: &[u32],
    installed: &HashSet<String>,
) -> Vec<SearchResult> {
    ids.iter()
        .filter_map(|id| rows.get(id))
        .map(|row| row_to_result(row, installed))
        .collect()
}

pub fn row_to_result(row: &PackageRow, installed: &HashSet<String>) -> SearchResult {
    let source = match row.source.as_str() {
        "aur" => PackageSource::Aur,
        _ => PackageSource::Repo,
    };
    SearchResult {
        name: row.name.clone(),
        source,
        description: row.description.clone(),
        version: row.version.clone(),
        repo: row.repo.clone(),
        installed: installed.contains(&row.name),
        num_votes: row.num_votes,
        popularity: row.popularity,
        last_update: row.last_update,
    }
}

pub fn apply_installed_to_results(results: &mut [SearchResult], installed: &HashSet<String>) {
    for result in results.iter_mut() {
        result.installed = installed.contains(&result.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, source: &str) -> PackageRow {
        PackageRow {
            name: name.to_string(),
            description: Some("desc".to_string()),
            source: source.to_string(),
            repo: Some("core".to_string()),
            version: "1.0-1".to_string(),
            last_update: Some(100),
            num_votes: None,
            popularity: None,
            package_base: Some(name.to_string()),
        }
    }

    #[test]
    fn preserves_ranked_id_order() {
        let mut rows = HashMap::new();
        rows.insert(3, row("gamma", "aur"));
        rows.insert(1, row("alpha", "aur"));
        rows.insert(2, row("beta", "aur"));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[3, 1, 2], &installed);
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "alpha", "beta"]);
    }

    #[test]
    fn maps_source_aur_and_repo() {
        let mut rows = HashMap::new();
        rows.insert(1, row("aur-pkg", "aur"));
        rows.insert(2, row("repo-pkg", "repo"));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[1, 2], &installed);
        assert_eq!(results[0].source, PackageSource::Aur);
        assert_eq!(results[1].source, PackageSource::Repo);
    }

    #[test]
    fn installed_flag_reflects_set_membership() {
        let mut rows = HashMap::new();
        rows.insert(1, row("vim", "aur"));
        rows.insert(2, row("emacs", "aur"));
        let mut installed = HashSet::new();
        installed.insert("vim".to_string());
        let results = to_search_results(&rows, &[1, 2], &installed);
        assert!(results[0].installed);
        assert!(!results[1].installed);
    }

    #[test]
    fn apply_installed_to_results_flips_only_members() {
        let mut rows = HashMap::new();
        rows.insert(1, row("vim", "aur"));
        rows.insert(2, row("emacs", "aur"));
        let empty = HashSet::new();
        let mut results = to_search_results(&rows, &[1, 2], &empty);
        let mut installed = HashSet::new();
        installed.insert("vim".to_string());
        apply_installed_to_results(&mut results, &installed);
        assert!(results[0].installed);
        assert!(!results[1].installed);
    }

    #[test]
    fn missing_id_is_skipped() {
        let mut rows = HashMap::new();
        rows.insert(1, row("alpha", "aur"));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[1, 99], &installed);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "alpha");
    }
}
