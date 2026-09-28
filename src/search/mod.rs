use std::path::{Path, PathBuf};

use crate::db::PackageDb;

use engine::SearchEngine;

pub mod engine {
    pub use pakajo_search::engine::SearchEngine;
}
pub mod hydrate;

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
mod cache_tests;
