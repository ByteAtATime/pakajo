use std::sync::Arc;

use crate::aur::{AurClient, AurInfo};
use crate::package::PackageSource;

use super::{SearchProvider, SearchQuery, SearchResult};

pub struct AurSearchProvider {
    client: Arc<AurClient>,
}

impl AurSearchProvider {
    pub fn new(client: Arc<AurClient>) -> Self {
        Self { client }
    }
}

impl SearchProvider for AurSearchProvider {
    fn name(&self) -> &'static str {
        "aur"
    }

    fn search(&self, q: &SearchQuery) -> anyhow::Result<Vec<SearchResult>> {
        if q.text.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let infos = self.client.search(&q.text, "name-desc")?;
        Ok(infos.into_iter().map(SearchResult::from).collect())
    }
}

impl From<AurInfo> for SearchResult {
    fn from(info: AurInfo) -> Self {
        SearchResult {
            name: info.name,
            source: PackageSource::Aur,
            description: info.description,
            version: info.version,
            repo: Some("aur".to_string()),
            num_votes: Some(info.num_votes),
            popularity: Some(info.popularity),
            installed: false,
            last_update: None,
            keywords: Vec::new(),
        }
    }
}
