use std::sync::Arc;

use crate::package::PackageSource;

use super::{SearchProvider, SearchQuery, SearchResult};

#[derive(Clone)]
pub struct RepoEntry {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub repo: String,
}

pub struct RepoSearchIndex {
    entries: Vec<RepoEntry>,
}

impl RepoSearchIndex {
    pub fn from_alpm(handle: &alpm::Alpm) -> Self {
        let mut entries = Vec::new();
        for db in handle.syncdbs().iter() {
            let repo = db.name().to_string();
            for pkg in db.pkgs().iter() {
                entries.push(RepoEntry {
                    name: pkg.name().to_string(),
                    version: pkg.version().to_string(),
                    description: pkg.desc().map(str::to_string),
                    repo: repo.clone(),
                });
            }
        }
        Self { entries }
    }

    #[cfg(test)]
    pub fn from_entries(entries: Vec<RepoEntry>) -> Self {
        Self { entries }
    }
}

pub struct RepoSearchProvider {
    index: Arc<RepoSearchIndex>,
}

impl RepoSearchProvider {
    pub fn new(index: Arc<RepoSearchIndex>) -> Self {
        Self { index }
    }
}

impl SearchProvider for RepoSearchProvider {
    fn name(&self) -> &'static str {
        "repo"
    }

    fn search(&self, q: &SearchQuery) -> anyhow::Result<Vec<SearchResult>> {
        if q.text.is_empty() {
            return Ok(Vec::new());
        }
        let needle = q.text.to_lowercase();
        let mut out: Vec<SearchResult> = Vec::new();
        for entry in &self.index.entries {
            let name_matches = entry.name.to_lowercase().contains(&needle);
            let desc_matches = entry
                .description
                .as_ref()
                .map(|d| d.to_lowercase().contains(&needle))
                .unwrap_or(false);
            if !name_matches && !desc_matches {
                continue;
            }
            out.push(SearchResult {
                source: PackageSource::Repo,
                name: entry.name.clone(),
                version: entry.version.clone(),
                description: entry.description.clone(),
                repo: Some(entry.repo.clone()),
                num_votes: None,
                popularity: None,
                installed: false,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, repo: &str, description: Option<&str>) -> RepoEntry {
        RepoEntry {
            name: name.to_string(),
            version: "1.0-1".to_string(),
            description: description.map(str::to_string),
            repo: repo.to_string(),
        }
    }

    #[test]
    fn matches_name_and_description_substring() {
        let index = RepoSearchIndex::from_entries(vec![
            entry("vim", "extra", Some("Vi IMproved editor")),
            entry("emacs", "extra", Some("an extensible editor")),
            entry("nano", "core", Some("simple text editor")),
        ]);
        let provider = RepoSearchProvider::new(Arc::new(index));

        let rows = provider
            .search(&SearchQuery::new("vim"))
            .expect("repo search");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "vim");
        assert_eq!(rows[0].repo.as_deref(), Some("extra"));
        assert_eq!(rows[0].source, PackageSource::Repo);

        let rows = provider
            .search(&SearchQuery::new("extensible"))
            .expect("repo search");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "emacs");
    }

    #[test]
    fn match_is_case_insensitive() {
        let index = RepoSearchIndex::from_entries(vec![entry("Firefox", "extra", None)]);
        let provider = RepoSearchProvider::new(Arc::new(index));

        let rows = provider
            .search(&SearchQuery::new("FIRE"))
            .expect("repo search");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Firefox");
    }

    #[test]
    fn empty_query_returns_empty() {
        let index = RepoSearchIndex::from_entries(vec![entry("vim", "extra", None)]);
        let provider = RepoSearchProvider::new(Arc::new(index));

        let rows = provider.search(&SearchQuery::new("")).expect("repo search");
        assert!(rows.is_empty());
    }

    #[test]
    fn provider_name_is_repo() {
        let index = RepoSearchIndex::from_entries(vec![]);
        let provider = RepoSearchProvider::new(Arc::new(index));
        assert_eq!(provider.name(), "repo");
    }
}
