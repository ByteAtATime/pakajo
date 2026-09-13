use std::collections::HashSet;

use crate::db::PackageRow;
use crate::package::PackageSource;

use super::SearchResult;

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
    fn maps_source_aur_and_repo() {
        let installed = HashSet::new();
        assert_eq!(
            row_to_result(&row("aur-pkg", "aur"), &installed).source,
            PackageSource::Aur
        );
        assert_eq!(
            row_to_result(&row("repo-pkg", "repo"), &installed).source,
            PackageSource::Repo
        );
    }

    #[test]
    fn apply_installed_to_results_flips_only_members() {
        let empty = HashSet::new();
        let mut results = vec![
            row_to_result(&row("vim", "aur"), &empty),
            row_to_result(&row("emacs", "aur"), &empty),
        ];
        let mut installed = HashSet::new();
        installed.insert("vim".to_string());
        apply_installed_to_results(&mut results, &installed);
        assert!(results[0].installed);
        assert!(!results[1].installed);
    }
}
