use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::local_index::LocalIndex;
use crate::package::PackageSource;

pub mod aur;
pub mod local;
pub mod ranking;
pub mod repo;

pub use aur::AurSearchProvider;
#[allow(unused_imports)]
pub use local::LocalSearchProvider;
pub use repo::{RepoSearchIndex, RepoSearchProvider};

pub struct SearchQuery {
    pub text: String,
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: Self::DEFAULT_LIMIT,
        }
    }

    const DEFAULT_LIMIT: usize = 50;
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            limit: Self::DEFAULT_LIMIT,
        }
    }
}

pub struct SearchResult {
    pub name: String,
    pub source: PackageSource,
    pub description: Option<String>,
    pub version: String,
    pub repo: Option<String>,
    pub num_votes: Option<u64>,
    pub popularity: Option<f64>,
    #[allow(dead_code)]
    pub installed: bool,
    #[allow(dead_code)]
    pub last_update: Option<i64>,
}

pub trait SearchProvider: Send + Sync {
    fn search(&self, q: &SearchQuery) -> anyhow::Result<Vec<SearchResult>>;
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
}

pub(crate) struct SearchOutcome {
    pub results: Vec<SearchResult>,
    pub aur_error: Option<String>,
}

pub(crate) fn execute_search(
    repo: &RepoSearchProvider,
    aur: &AurSearchProvider,
    text: &str,
) -> SearchOutcome {
    let q = SearchQuery::new(text);
    let repo_rows = repo.search(&q).expect("something went very wrong");
    let mut ok_rows: Vec<Vec<SearchResult>> = vec![repo_rows];
    let aur_error = match aur.search(&q) {
        Ok(rows) => {
            ok_rows.push(rows);
            None
        }
        Err(err) => Some(friendly_search_error(&err)),
    };
    let results = merge_and_rank(ok_rows, &q);
    SearchOutcome { results, aur_error }
}

pub(crate) fn dispatch_search(
    local: Option<Arc<LocalIndex>>,
    repo: &RepoSearchProvider,
    aur: &AurSearchProvider,
    installed: &HashSet<String>,
    text: &str,
) -> SearchOutcome {
    let q = SearchQuery::new(text);
    if let Some(index) = local
        && index.is_populated()
    {
        let provider = LocalSearchProvider::new(index);
        if let Ok(candidates) = provider.search(&q) {
            let results = ranking::score(candidates, &q, installed);
            return SearchOutcome {
                results,
                aur_error: None,
            };
        }
    }
    execute_search(repo, aur, text)
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

pub fn merge_and_rank(per_provider: Vec<Vec<SearchResult>>, q: &SearchQuery) -> Vec<SearchResult> {
    if q.text.is_empty() {
        return Vec::new();
    }

    let flattened: Vec<SearchResult> = per_provider.into_iter().flatten().collect();

    let mut deduped: Vec<SearchResult> = Vec::with_capacity(flattened.len());
    let mut index_by_name: HashMap<String, usize> = HashMap::new();
    for result in flattened {
        match index_by_name.get(&result.name) {
            None => {
                index_by_name.insert(result.name.clone(), deduped.len());
                deduped.push(result);
            }
            Some(&idx) => {
                let existing_is_aur = deduped[idx].source == PackageSource::Aur;
                let incoming_is_repo = result.source == PackageSource::Repo;
                if existing_is_aur && incoming_is_repo {
                    deduped[idx] = result;
                }
            }
        }
    }

    let needle = q.text.to_lowercase();
    deduped.sort_by(|a, b| {
        match tier_of(a, &needle).cmp(&tier_of(b, &needle)) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
        match popularity_of(b).partial_cmp(&popularity_of(a)) {
            Some(std::cmp::Ordering::Equal) | None => {}
            Some(ordering) => return ordering,
        }
        match votes_of(b).cmp(&votes_of(a)) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
        a.name.cmp(&b.name)
    });

    deduped.truncate(q.limit);
    deduped
}

fn tier_of(result: &SearchResult, needle: &str) -> u8 {
    let name = result.name.to_lowercase();
    if name == needle {
        return 0;
    }
    if name.starts_with(needle) {
        return 1;
    }
    if name.contains(needle) {
        return 2;
    }
    if result
        .description
        .as_ref()
        .map(|d| d.to_lowercase().contains(needle))
        .unwrap_or(false)
    {
        return 3;
    }
    4
}

fn popularity_of(result: &SearchResult) -> f64 {
    result.popularity.unwrap_or(0.0)
}

fn votes_of(result: &SearchResult) -> u64 {
    result.num_votes.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageSource;

    fn make(
        name: &str,
        source: PackageSource,
        popularity: Option<f64>,
        votes: Option<u64>,
        description: Option<&str>,
    ) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            source,
            description: description.map(str::to_string),
            version: "1.0-1".to_string(),
            repo: Some("core".to_string()),
            num_votes: votes,
            popularity,
            installed: false,
            last_update: None,
        }
    }

    #[test]
    fn exact_ranks_above_prefix_above_substring() {
        let rows = vec![
            make("evimeline", PackageSource::Repo, None, None, None),
            make("vim-plugins", PackageSource::Repo, None, None, None),
            make("vim", PackageSource::Repo, None, None, None),
        ];
        let ranked = merge_and_rank(vec![rows], &SearchQuery::new("vim"));
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].name, "vim");
        assert_eq!(ranked[1].name, "vim-plugins");
        assert_eq!(ranked[2].name, "evimeline");
    }

    #[test]
    fn dedup_prefers_repo_on_name_collision() {
        let aur_rows = vec![make("foo", PackageSource::Aur, Some(9.0), Some(99), None)];
        let repo_rows = vec![make("foo", PackageSource::Repo, None, None, None)];
        let ranked = merge_and_rank(vec![aur_rows, repo_rows], &SearchQuery::new("foo"));
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].source, PackageSource::Repo);
    }

    #[test]
    fn dedup_keeps_first_when_same_source() {
        let rows_a = vec![make("foo", PackageSource::Aur, Some(1.0), None, None)];
        let rows_b = vec![make("foo", PackageSource::Aur, Some(9.0), None, None)];
        let ranked = merge_and_rank(vec![rows_a, rows_b], &SearchQuery::new("foo"));
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].popularity, Some(1.0));
    }

    #[test]
    fn popularity_tiebreaks_within_tier() {
        let rows = vec![
            make("bar", PackageSource::Repo, Some(5.0), None, None),
            make("baz", PackageSource::Repo, Some(9.0), None, None),
        ];
        let ranked = merge_and_rank(vec![rows], &SearchQuery::new("a"));
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].name, "baz");
        assert_eq!(ranked[1].name, "bar");
    }

    #[test]
    fn votes_tiebreak_after_popularity() {
        let rows = vec![
            make("bar", PackageSource::Repo, Some(5.0), Some(3), None),
            make("baz", PackageSource::Repo, Some(5.0), Some(10), None),
        ];
        let ranked = merge_and_rank(vec![rows], &SearchQuery::new("a"));
        assert_eq!(ranked[0].name, "baz");
        assert_eq!(ranked[1].name, "bar");
    }

    #[test]
    fn name_alphabetical_is_final_tiebreak() {
        let rows = vec![
            make("zebra", PackageSource::Repo, Some(5.0), Some(10), None),
            make("alpha", PackageSource::Repo, Some(5.0), Some(10), None),
        ];
        let ranked = merge_and_rank(vec![rows], &SearchQuery::new("a"));
        assert_eq!(ranked[0].name, "alpha");
        assert_eq!(ranked[1].name, "zebra");
    }

    #[test]
    fn limit_truncates() {
        let rows = vec![
            make("alpha", PackageSource::Repo, None, None, None),
            make("alphabet", PackageSource::Repo, None, None, None),
            make("alphabeta", PackageSource::Repo, None, None, None),
        ];
        let q = SearchQuery {
            text: "a".into(),
            limit: 2,
        };
        let ranked = merge_and_rank(vec![rows], &q);
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn empty_query_returns_empty() {
        let rows = vec![make("anything", PackageSource::Repo, None, None, None)];
        let ranked = merge_and_rank(vec![rows], &SearchQuery::new(""));
        assert!(ranked.is_empty());
    }

    #[test]
    fn default_limit_is_fifty() {
        assert_eq!(SearchQuery::default().limit, 50);
    }

    #[test]
    fn dispatch_uses_local_when_populated() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = Arc::new(crate::local_index::LocalIndex::open(&path).expect("open"));
        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                "google-chrome",
                "aur",
                "aur",
                "1.0-1",
                "browser",
                0i64,
                0.0f64,
                0i64,
                "google-chrome"
            ],
        )
        .expect("seed packages");
        conn.execute(
            "INSERT INTO packages_fts \
             (name,description,source,repo,version,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                "google-chrome",
                "browser",
                "aur",
                "aur",
                "1.0-1",
                0i64,
                0.0f64,
                0i64,
                "google-chrome"
            ],
        )
        .expect("seed packages_fts");

        let empty_installed: HashSet<String> = HashSet::new();
        let repo_provider =
            RepoSearchProvider::new(Arc::new(RepoSearchIndex::from_entries(Vec::new())));
        let aur_provider = AurSearchProvider::new(Arc::new(crate::aur::AurClient::new()));

        let outcome = dispatch_search(
            Some(index.clone()),
            &repo_provider,
            &aur_provider,
            &empty_installed,
            "google-chrome",
        );

        let names: Vec<&str> = outcome.results.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"google-chrome"),
            "local branch should surface seeded package; got {names:?}"
        );
        assert!(
            outcome.aur_error.is_none(),
            "local branch must not record an aur error"
        );
        assert!(index.is_populated(), "seeded index reports populated");
    }
}
