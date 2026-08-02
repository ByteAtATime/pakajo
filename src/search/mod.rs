use std::collections::HashSet;

use crate::local_index::LocalIndex;
use crate::package::PackageSource;

use engine::SearchEngine;

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
    #[allow(dead_code)]
    pub last_update: Option<i64>,
}

pub(crate) fn dispatch_search(
    engine: &SearchEngine,
    sqlite: &LocalIndex,
    installed: &HashSet<String>,
    text: &str,
) -> Vec<SearchResult> {
    let _span = perf::PerfSpan::new("search");
    let ids = engine.search(text);
    if ids.is_empty() {
        return Vec::new();
    }
    let rows = match sqlite.hydrate_by_ids(&ids) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    hydrate::to_search_results(&rows, &ids, installed)
}

pub(crate) fn friendly_search_error(err: &anyhow::Error) -> String {
    let msg = format!("{err:#}");
    if msg.contains("Too many package results") {
        "Too many results! Please narrow your search".to_string()
    } else {
        msg.strip_prefix("AUR RPC error: ")
            .unwrap_or(&msg)
            .to_string()
    }
}
