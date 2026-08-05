use std::collections::HashSet;

use crate::local_index::LocalIndex;
use crate::package::PackageSource;

use engine::SearchEngine;
use tiers::Tier;

pub mod engine;
pub mod fuzzy;
pub mod hydrate;
pub mod index;
pub mod perf;
pub mod query;
pub mod tiers;

pub struct SearchResult {
    pub name: String,
    pub source: PackageSource,
    pub description: Option<String>,
    pub version: String,
    pub repo: Option<String>,
    pub installed: bool,
    pub num_votes: Option<i64>,
    pub popularity: Option<f64>,
    #[allow(dead_code)]
    pub last_update: Option<i64>,
}

pub fn dispatch_search(
    engine: &SearchEngine,
    sqlite: &LocalIndex,
    installed: &HashSet<String>,
    text: &str,
    group_index: &[(String, String)],
) -> Vec<SearchResult> {
    let _span = perf::PerfSpan::new("search");
    let pairs = engine.search_tiered(text);
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
    if !q.is_empty() {
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

fn group_search_result(name: &String, repo: &String) -> SearchResult {
    SearchResult {
        name: name.clone(),
        source: PackageSource::Group,
        description: Some("group".to_string()),
        version: String::new(),
        repo: Some(repo.clone()),
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
    use crate::search::tiers::Tier;

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
