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
        let limit = dynamic_limit(&q.text);
        let rows = if q.text.chars().count() <= 2 {
            self.index.search_name_prefix_ranked(&q.text, limit)
        } else {
            self.index.search(&pattern, limit)
        }?;
        Ok(rows.into_iter().map(row_to_result).collect())
    }
}

fn build_match(query: &str) -> Option<String> {
    let name_only = query.chars().count() <= 2;
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
        .map(|t| {
            let star = format!("{}*", t.to_lowercase());
            if name_only {
                format!("name:{star}")
            } else {
                star
            }
        })
        .collect();
    Some(lower.join(" OR "))
}

fn dynamic_limit(query: &str) -> i64 {
    match query.chars().count() {
        2 => 250,
        _ => 5000,
    }
}

pub(super) fn row_to_result(row: PackageRow) -> SearchResult {
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
        keywords: row.keywords,
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

    #[test]
    fn dynamic_limit_two_chars_returns_two_hundred_fifty() {
        assert_eq!(dynamic_limit("ca"), 250);
    }

    #[test]
    fn dynamic_limit_three_chars_returns_default_cap() {
        assert_eq!(dynamic_limit("cav"), 5000);
    }

    #[test]
    fn build_match_two_char_query_uses_name_column() {
        assert_eq!(build_match("ca").as_deref(), Some("name:ca*"));
    }

    #[test]
    fn build_match_three_char_query_skips_column_filter() {
        assert_eq!(build_match("cav").as_deref(), Some("cav*"));
    }

    #[test]
    fn two_char_exact_name_outranks_long_descriptions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let long_desc = "Tool that streams formatted report lines and session output for downstream analysis pipelines";
        let index = seed(
            &dir,
            &[
                ("sl", Some(long_desc)),
                ("sls", Some("x")),
                ("slang", Some("short")),
                ("slim", Some("short")),
                ("haskell-slist", Some("short")),
            ],
        );
        let provider = LocalSearchProvider::new(index);

        let rows = provider
            .search(&SearchQuery::new("sl"))
            .expect("local search");
        let got = names(&rows);

        assert!(!got.is_empty(), "sl* query should recall packages");
        assert_eq!(
            got[0], "sl",
            "exact two-char name must rank first despite a long description that buries it under bm25"
        );

        let haskell_pos = got
            .iter()
            .position(|n| *n == "haskell-slist")
            .expect("token-only-prefix name haskell-slist should be recalled");
        for expected in ["sl", "sls", "slang", "slim"] {
            let pos = got.iter().position(|n| *n == expected).unwrap_or_else(|| {
                panic!("literal-sl prefix {expected} should be recalled; got {got:?}")
            });
            assert!(
                pos < haskell_pos,
                "literal-sl prefix {expected} must precede the token-only-prefix name haskell-slist; got {got:?}"
            );
        }
    }

    #[test]
    fn two_char_input_with_fts_special_char_does_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = seed(
            &dir,
            &[
                ("apple", Some("a fruit")),
                ("abc", Some("the alphabet")),
                ("vim", Some("an editor")),
            ],
        );
        let provider = LocalSearchProvider::new(index);

        let rows = provider.search(&SearchQuery::new("a\"")).expect(
            "local search must not error on a two-char input carrying an FTS-special character",
        );
        let got = names(&rows);
        assert!(
            got.iter().any(|n| *n == "apple" || *n == "abc"),
            "sanitized token a should recall a* packages; got {got:?}"
        );

        let empty_rows = provider
            .search(&SearchQuery::new("@"))
            .expect("local search must not error on an input that sanitizes to empty");
        assert!(
            empty_rows.is_empty(),
            "input that sanitizes to empty must return no results; got {:?}",
            names(&empty_rows)
        );
    }
}
