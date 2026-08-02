use std::collections::{HashMap, HashSet};

use crate::local_index::PackageRow;
use crate::package::PackageSource;

use super::SearchResult;

pub fn to_search_results(
    rows: &HashMap<u32, PackageRow>,
    ids: &[u32],
    installed: &HashSet<String>,
) -> Vec<SearchResult> {
    ids.iter()
        .filter_map(|id| rows.get(id))
        .map(|row| {
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
                num_votes: row.num_votes.map(|v| v as u64),
                popularity: row.popularity,
                installed: installed.contains(&row.name),
                last_update: row.last_update,
                keywords: row.keywords.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, source: &str, num_votes: Option<i64>) -> PackageRow {
        PackageRow {
            name: name.to_string(),
            description: Some("desc".to_string()),
            source: source.to_string(),
            repo: Some("core".to_string()),
            version: "1.0-1".to_string(),
            num_votes,
            popularity: Some(1.0),
            last_update: Some(100),
            package_base: Some(name.to_string()),
            keywords: vec![],
        }
    }

    #[test]
    fn preserves_ranked_id_order() {
        let mut rows = HashMap::new();
        rows.insert(3, row("gamma", "aur", None));
        rows.insert(1, row("alpha", "aur", None));
        rows.insert(2, row("beta", "aur", None));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[3, 1, 2], &installed);
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "alpha", "beta"]);
    }

    #[test]
    fn maps_source_aur_and_repo() {
        let mut rows = HashMap::new();
        rows.insert(1, row("aur-pkg", "aur", None));
        rows.insert(2, row("repo-pkg", "repo", None));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[1, 2], &installed);
        assert_eq!(results[0].source, PackageSource::Aur);
        assert_eq!(results[1].source, PackageSource::Repo);
    }

    #[test]
    fn installed_flag_reflects_set_membership() {
        let mut rows = HashMap::new();
        rows.insert(1, row("vim", "aur", None));
        rows.insert(2, row("emacs", "aur", None));
        let mut installed = HashSet::new();
        installed.insert("vim".to_string());
        let results = to_search_results(&rows, &[1, 2], &installed);
        assert!(results[0].installed);
        assert!(!results[1].installed);
    }

    #[test]
    fn missing_id_is_skipped() {
        let mut rows = HashMap::new();
        rows.insert(1, row("alpha", "aur", None));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[1, 99], &installed);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "alpha");
    }

    #[test]
    fn num_votes_casts_i64_to_u64() {
        let mut rows = HashMap::new();
        rows.insert(1, row("voted", "aur", Some(42)));
        let installed = HashSet::new();
        let results = to_search_results(&rows, &[1], &installed);
        assert_eq!(results[0].num_votes, Some(42));
    }
}
