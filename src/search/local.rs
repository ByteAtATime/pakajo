use std::sync::Arc;

use crate::local_index::{LocalIndex, PackageRow};
use crate::package::PackageSource;

use super::{SearchProvider, SearchQuery, SearchResult};

pub struct LocalSearchProvider {
    index: Arc<LocalIndex>,
}

impl LocalSearchProvider {
    #[allow(dead_code)]
    pub fn new(index: Arc<LocalIndex>) -> Self {
        Self { index }
    }
}

impl SearchProvider for LocalSearchProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    fn search(&self, q: &SearchQuery) -> anyhow::Result<Vec<SearchResult>> {
        let Some(pattern) = build_match(&q.text) else {
            return Ok(Vec::new());
        };
        let rows = self.index.search(&pattern, 5000)?;
        Ok(rows.into_iter().map(row_to_result).collect())
    }
}

fn build_match(query: &str) -> Option<String> {
    let mut tokens: Vec<&str> = Vec::new();
    for token in query.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        tokens.push(token);
    }
    if tokens.is_empty() {
        return None;
    }
    let lower: Vec<String> = tokens
        .iter()
        .map(|t| format!("{}*", t.to_lowercase()))
        .collect();
    Some(lower.join(" OR "))
}

fn row_to_result(row: PackageRow) -> SearchResult {
    let source = match row.source.as_str() {
        "aur" => PackageSource::Aur,
        _ => PackageSource::Repo,
    };
    SearchResult {
        name: row.name,
        source,
        description: row.description,
        version: row.version,
        repo: row.repo,
        num_votes: row.num_votes.map(|v| v as u64),
        popularity: row.popularity,
        installed: false,
        last_update: row.last_update,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{SearchProvider, SearchQuery};

    fn seed(dir: &tempfile::TempDir, rows: &[(&str, Option<&str>)]) -> Arc<LocalIndex> {
        let index =
            Arc::new(LocalIndex::open(&dir.path().join("aur-meta.sqlite")).expect("open index"));
        let conn = rusqlite::Connection::open(dir.path().join("aur-meta.sqlite")).expect("seed");
        for (name, desc) in rows {
            conn.execute(
                "INSERT INTO packages \
                 (name,description,source,repo,version,num_votes,popularity,last_update,package_base) \
                 VALUES (?,?,?,?,?,?,?,?,?)",
                rusqlite::params![name, desc, "aur", "aur", "1.0-1", 0i64, 0.0f64, 0i64, name],
            )
            .expect("seed insert");
        }
        index
    }

    fn names(rows: &[SearchResult]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn recall_matches_token_prefix_in_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(
            &dir,
            &[
                ("google-chrome", Some("browser")),
                ("chromium", Some("open source browser")),
                ("firefox", Some("another browser")),
            ],
        );
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new("chrom"))
            .expect("local search");
        let mut got = names(&rows);
        got.sort_unstable();
        assert_eq!(got, vec!["chromium", "google-chrome"]);
    }

    #[test]
    fn query_with_dash_is_split_into_tokens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(
            &dir,
            &[
                ("google-chrome", Some("browser")),
                ("chromium", Some("browser")),
            ],
        );
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new("google-chrome"))
            .expect("local search");
        assert!(
            names(&rows).contains(&"google-chrome"),
            "google-chrome should be recalled; got {:?}",
            names(&rows)
        );
    }

    #[test]
    fn empty_query_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(&dir, &[("vim", None)]);
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new(""))
            .expect("local search");
        assert!(rows.is_empty());
    }

    #[test]
    fn punctuation_only_query_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(&dir, &[("vim", None)]);
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new("--"))
            .expect("local search");
        assert!(rows.is_empty());
    }

    #[test]
    fn description_only_match_is_recalled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(
            &dir,
            &[
                ("vim", Some("Vi IMproved editor")),
                ("unrelated", Some("a game")),
            ],
        );
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new("editor"))
            .expect("local search");
        assert_eq!(names(&rows), vec!["vim"]);
    }

    #[test]
    fn provider_name_is_local() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(&dir, &[]);
        let provider = LocalSearchProvider::new(index);
        assert_eq!(provider.name(), "local");
    }

    #[test]
    fn build_match_single_token_appends_prefix_star() {
        assert_eq!(build_match("vim").as_deref(), Some("vim*"));
    }

    #[test]
    fn build_match_splits_on_non_alphanumeric() {
        assert_eq!(
            build_match("google-chrome").as_deref(),
            Some("google* OR chrome*")
        );
    }

    #[test]
    fn build_match_drops_punctuation_only_tokens() {
        assert_eq!(build_match("vim!").as_deref(), Some("vim*"));
    }

    #[test]
    fn build_match_returns_none_for_empty() {
        assert!(build_match("").is_none());
    }

    #[test]
    fn build_match_returns_none_for_punctuation_only() {
        assert!(build_match("--").is_none());
        assert!(build_match("!@#").is_none());
    }
}
