use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::db::PackageDb;
use crate::package::PackageSource;

use engine::SearchEngine;
use tiers::Tier;

pub mod engine {
    pub use pakajo_search::engine::SearchEngine;
}
pub mod hydrate;
pub mod tiers {
    pub use pakajo_search::tiers::Tier;
}

pub use hydrate::{apply_installed_to_results, hydrate_metas};
pub use pakajo_search::SearchFilter;

pub fn cache_path(sqlite: &Path) -> PathBuf {
    sqlite.with_file_name("index.bin")
}

pub fn engine_for(
    db: &PackageDb,
    cache: &Path,
) -> Result<SearchEngine, pakajo_search::SearchError> {
    let fingerprint = crate::db::db_cache_fingerprint(db);
    match SearchEngine::build(Some((cache, fingerprint)), || db.index_rows()) {
        Err(pakajo_search::SearchError::Corrupt) => {
            eprintln!("search cache corrupt, rebuilding");
            let _ = std::fs::remove_file(cache);
            SearchEngine::build(Some((cache, fingerprint)), || db.index_rows())
        }
        other => other,
    }
}

#[cfg(test)]
mod engine_tests;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub name: String,
    pub source: PackageSource,
    pub description: Option<String>,
    pub version: String,
    pub repo: Option<String>,
    pub installed: bool,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
    pub last_update: Option<i64>,
}

pub fn dispatch_search(
    engine: &SearchEngine,
    sqlite: &PackageDb,
    installed: &HashSet<String>,
    text: &str,
    group_index: &[(String, String)],
    filter: SearchFilter,
) -> Vec<SearchResult> {
    let lowered: Option<HashSet<String>> = if filter == SearchFilter::Installed {
        Some(installed.iter().map(|name| name.to_lowercase()).collect())
    } else {
        None
    };
    let engine_installed = lowered.as_ref().unwrap_or(installed);
    let pairs = engine.search_tiered(text, filter, engine_installed);
    let ids: Vec<u32> = pairs.iter().map(|(id, _)| *id).collect();
    let mut entries: Vec<(Tier, SearchResult)> = Vec::with_capacity(pairs.len());
    if !ids.is_empty()
        && let Ok(rows) = sqlite.hydrate_by_ids(&ids)
    {
        for (id, tier) in &pairs {
            if let Some(row) = rows.get(id) {
                entries.push((*tier, hydrate::row_to_result(row, installed)));
            }
        }
    }
    let q = text.to_lowercase();
    if !q.is_empty() && filter == SearchFilter::All {
        for (name, repo) in group_index {
            if let Some(tier) = group_name_tier(&name.to_lowercase(), &q) {
                entries.push((tier, group_search_result(name, repo)));
            }
        }
    }
    entries.sort_by_key(|a| a.0);
    entries.into_iter().map(|(_, r)| r).collect()
}

fn group_name_tier(name: &str, q: &str) -> Option<Tier> {
    if name == q {
        Some(Tier::ExactName)
    } else if name.starts_with(q) {
        Some(Tier::PrefixName)
    } else if name.contains(q) {
        Some(Tier::Substring)
    } else {
        None
    }
}

fn group_search_result(name: &str, repo: &str) -> SearchResult {
    SearchResult {
        name: name.to_owned(),
        source: PackageSource::Group,
        description: Some("group".to_string()),
        version: String::new(),
        repo: Some(repo.to_owned()),
        installed: false,
        num_votes: None,
        popularity: None,
        last_update: None,
    }
}

pub fn friendly_search_error(err: &anyhow::Error) -> String {
    let msg = format!("{err:#}");
    if msg.contains("Too many package results") {
        "Too many results! Please narrow your search".to_string()
    } else {
        msg.strip_prefix("AUR RPC error: ")
            .unwrap_or(&msg)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_name_tier_classifies_exact_prefix_substring() {
        assert_eq!(group_name_tier("gnome", "gnome"), Some(Tier::ExactName));
        assert_eq!(
            group_name_tier("gnome-shell", "gnome"),
            Some(Tier::PrefixName)
        );
        assert_eq!(group_name_tier("x-gnome-y", "gnome"), Some(Tier::Substring));
        assert_eq!(group_name_tier("kde", "gnome"), None);
    }
}
