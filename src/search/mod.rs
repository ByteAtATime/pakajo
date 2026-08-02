use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::local_index::LocalIndex;
use crate::package::PackageSource;

pub mod aur;
pub mod fuzzy;
pub mod index;
pub mod local;
pub mod perf;
pub mod query;
pub mod ranking;
pub mod repo;
pub mod tiers;

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
    pub installed: bool,
    #[allow(dead_code)]
    pub last_update: Option<i64>,
    pub keywords: Vec<String>,
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
    let one_char_alnum = text.chars().count() == 1
        && text
            .chars()
            .next()
            .map(char::is_alphanumeric)
            .unwrap_or(false);
    let mut outcome =
        if one_char_alnum && let Some(index) = local.as_ref().filter(|i| i.is_populated()) {
            match index.search_recent_repo_prefix(text, 50) {
                Ok(rows) => SearchOutcome {
                    results: rows.into_iter().map(local::row_to_result).collect(),
                    aur_error: None,
                },
                Err(_) => execute_search(repo, aur, text),
            }
        } else if let Some(index) = local
            && index.is_populated()
        {
            let provider = LocalSearchProvider::new(index);
            if let Ok(candidates) = provider.search(&q) {
                SearchOutcome {
                    results: ranking::score(candidates, &q, installed),
                    aur_error: None,
                }
            } else {
                execute_search(repo, aur, text)
            }
        } else {
            execute_search(repo, aur, text)
        };
    for result in outcome.results.iter_mut() {
        result.installed = installed.contains(&result.name);
    }
    outcome
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
            keywords: vec![],
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

    #[test]
    fn dispatch_sets_installed_flag() {
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
                "vim", "aur", "aur", "1.0-1", "editor", 0i64, 0.0f64, 0i64, "vim"
            ],
        )
        .expect("seed packages vim");
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                "vim-plugins",
                "aur",
                "aur",
                "1.0-1",
                "vim addons",
                0i64,
                0.0f64,
                0i64,
                "vim-plugins"
            ],
        )
        .expect("seed packages vim-plugins");

        let installed: HashSet<String> = HashSet::from(["vim".to_string()]);
        let repo_provider =
            RepoSearchProvider::new(Arc::new(RepoSearchIndex::from_entries(Vec::new())));
        let aur_provider = AurSearchProvider::new(Arc::new(crate::aur::AurClient::new()));

        let outcome = dispatch_search(
            Some(index.clone()),
            &repo_provider,
            &aur_provider,
            &installed,
            "vim",
        );

        let vim_result = outcome
            .results
            .iter()
            .find(|r| r.name == "vim")
            .expect("vim row present");
        assert!(
            vim_result.installed,
            "vim is in installed set; flag must be true"
        );
        let plugins_result = outcome
            .results
            .iter()
            .find(|r| r.name == "vim-plugins")
            .expect("vim-plugins row present");
        assert!(
            !plugins_result.installed,
            "vim-plugins is not in installed set; flag must be false"
        );
    }

    #[test]
    fn dispatch_search_one_char_uses_recent_repo_prefix_branch() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = Arc::new(crate::local_index::LocalIndex::open(&path).expect("open"));
        let conn = rusqlite::Connection::open(&path).expect("seed");

        let insert = |name: &str, source: &str, last_update: i64| {
            conn.execute(
                "INSERT INTO packages (name, source, repo, version, description, num_votes, popularity, last_update, package_base) \
                 VALUES (?, ?, 'repo', '1', NULL, NULL, NULL, ?, NULL)",
                rusqlite::params![name, source, last_update],
            )
            .expect("seed");
        };
        insert("chromium", "repo", 1000);
        insert("cake", "repo", 500);
        insert("c-aur", "aur", 9999);
        insert("firefox", "repo", 9999);

        let empty_installed: HashSet<String> = HashSet::new();
        let repo_provider =
            RepoSearchProvider::new(Arc::new(RepoSearchIndex::from_entries(Vec::new())));
        let aur_provider = AurSearchProvider::new(Arc::new(crate::aur::AurClient::new()));

        let outcome = dispatch_search(
            Some(index.clone()),
            &repo_provider,
            &aur_provider,
            &empty_installed,
            "c",
        );

        let names: Vec<&str> = outcome.results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["chromium", "cake"]);
        assert!(
            outcome
                .results
                .iter()
                .all(|r| r.source == PackageSource::Repo),
            "AUR package c-aur must not appear in 1-char recent-repo-prefix results"
        );
    }

    #[test]
    fn dispatch_search_one_char_non_alphanumeric_falls_through() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = Arc::new(crate::local_index::LocalIndex::open(&path).expect("open"));
        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute(
            "INSERT INTO packages (name, source, repo, version, description, num_votes, popularity, last_update, package_base) \
             VALUES ('chromium', 'repo', 'extra', '1', NULL, NULL, NULL, 1000, NULL)",
            [],
        )
        .expect("seed");

        let empty_installed: HashSet<String> = HashSet::new();
        let repo_provider =
            RepoSearchProvider::new(Arc::new(RepoSearchIndex::from_entries(Vec::new())));
        let aur_provider = AurSearchProvider::new(Arc::new(crate::aur::AurClient::new()));

        let outcome = dispatch_search(
            Some(index),
            &repo_provider,
            &aur_provider,
            &empty_installed,
            "?",
        );
        assert!(outcome.results.is_empty());
    }
}
