use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::package::PackageSource;
use crate::search::{SearchQuery, SearchResult};

#[allow(dead_code)]
pub struct ScoredCandidate {
    pub result: SearchResult,
    pub name_score: Option<u16>,
    pub desc_score: Option<u16>,
    pub keyword_score: Option<u16>,
}

#[allow(dead_code)]
pub fn scored_candidates(candidates: Vec<SearchResult>, needle: &str) -> Vec<ScoredCandidate> {
    let mut cfg = Config::DEFAULT;
    cfg.prefer_prefix = true;
    let mut matcher = Matcher::new(cfg);
    let tokens: Vec<String> = needle.split_whitespace().map(str::to_lowercase).collect();
    let mut hbuf: Vec<char> = Vec::new();
    let mut nbuf: Vec<char> = Vec::new();
    let mut match_field = |haystack: &str| -> Option<u16> {
        if tokens.is_empty() {
            return None;
        }
        hbuf.clear();
        let haystack_utf32 = Utf32Str::new(haystack, &mut hbuf);
        let mut sum: u16 = 0;
        for token in &tokens {
            nbuf.clear();
            let token_utf32 = Utf32Str::new(token, &mut nbuf);
            match matcher.fuzzy_match(haystack_utf32, token_utf32) {
                Some(s) => sum += s,
                None => return None,
            }
        }
        Some(sum)
    };
    candidates
        .into_iter()
        .map(|result| {
            let name_score = match_field(&result.name);
            let desc_score = result
                .description
                .as_ref()
                .and_then(|d| match_field(d));
            let keyword_score = if result.keywords.is_empty() {
                None
            } else {
                let mut best: Option<u16> = None;
                for kw in &result.keywords {
                    if let Some(sc) = match_field(kw) {
                        best = Some(best.map_or(sc, |b| b.max(sc)));
                    }
                }
                best
            };
            ScoredCandidate {
                result,
                name_score,
                desc_score,
                keyword_score,
            }
        })
        .collect()
}

const W_NAME: f64 = 1.0;
const W_DESC: f64 = 0.3;
const W_KW: f64 = 0.5;
const W_POP: f64 = 0.2;
const W_REPO: f64 = 0.1;
const W_RECENCY: f64 = 0.1;
const W_INSTALLED: f64 = 0.5;
const K_NAME: f64 = 100.0;
const K_DESC: f64 = 100.0;
const K_KW: f64 = 100.0;
const K_POP: f64 = 10.0;

#[allow(dead_code)]
pub fn score(
    candidates: Vec<SearchResult>,
    q: &SearchQuery,
    installed: &HashSet<String>,
) -> Vec<SearchResult> {
    if q.text.is_empty() {
        return Vec::new();
    }
    let needle = q.text.to_lowercase();
    let now = now_secs();
    let scored = scored_candidates(dedup_keep_repo(candidates), &needle);
    let has_name_match = scored.iter().any(|c| c.name_score.is_some());
    let filtered: Vec<ScoredCandidate> = if has_name_match {
        scored
            .into_iter()
            .filter(|c| c.name_score.is_some() || c.keyword_score.is_some())
            .collect()
    } else {
        scored
    };
    let mut ranked: Vec<(f64, SearchResult)> = filtered
        .into_iter()
        .map(|c| {
            let s = composite(
                &c.result,
                &needle,
                c.name_score,
                c.desc_score,
                c.keyword_score,
                installed,
                now,
            );
            (s, c.result)
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.name.cmp(&b.1.name))
    });

    ranked.into_iter().map(|(_, r)| r).take(q.limit).collect()
}

fn dedup_keep_repo(rows: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<SearchResult> = Vec::with_capacity(rows.len());

    for incoming in rows {
        match index.get(&incoming.name).copied() {
            None => {
                index.insert(incoming.name.clone(), out.len());
                out.push(incoming);
            }
            Some(i)
                if out[i].source == PackageSource::Aur
                    && incoming.source == PackageSource::Repo =>
            {
                out[i] = incoming
            }
            _ => {}
        }
    }
    out
}

fn saturate(x: f64, k: f64) -> f64 {
    x / (x + k)
}

fn composite(
    r: &SearchResult,
    needle: &str,
    name_score: Option<u16>,
    desc_score: Option<u16>,
    keyword_score: Option<u16>,
    installed: &HashSet<String>,
    now: f64,
) -> f64 {
    let needle_len = needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .count() as f64;
    let name_len = r.name.chars().count() as f64;
    let precision = (needle_len / name_len).min(1.0);

    let norm_name = name_score
        .map(|s| saturate(s as f64, K_NAME))
        .unwrap_or(0.0);
    let norm_desc = desc_score
        .map(|s| saturate(s as f64, K_DESC))
        .unwrap_or(0.0);
    let norm_kw = keyword_score
        .map(|s| saturate(s as f64, K_KW))
        .unwrap_or(0.0);
    let norm_pop = r.popularity.map(|p| saturate(p, K_POP)).unwrap_or(0.0);

    let text = norm_name * precision
        + W_KW * norm_kw * (1.0 - norm_name)
        + W_DESC * norm_desc * (1.0 - norm_name.max(norm_kw));

    let recency = if r.source == PackageSource::Aur {
        r.last_update
            .map(|t| 1.0 / (1.0 + ((now - t as f64).max(0.0) / 2_592_000.0).ln_1p()))
            .unwrap_or(1.0)
    } else {
        1.0
    };

    let repo_bonus = if r.source == PackageSource::Repo {
        W_REPO
    } else {
        0.0
    };
    let installed_bonus = if installed.contains(&r.name) {
        W_INSTALLED
    } else {
        0.0
    };

    W_NAME * text + W_RECENCY * recency + W_POP * norm_pop + repo_bonus + installed_bonus
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageSource;

    fn row(name: &str, description: Option<&str>) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            source: PackageSource::Aur,
            description: description.map(str::to_string),
            version: "1.0-1".to_string(),
            repo: Some("aur".to_string()),
            num_votes: None,
            popularity: None,
            installed: false,
            last_update: None,
            keywords: vec![],
        }
    }

    #[test]
    fn pre_lowercase_needle_matches_capitalized_name() {
        let scored = scored_candidates(vec![row("Firefox", None)], "fire");
        assert_eq!(scored.len(), 1);
        assert!(
            scored[0].name_score.is_some(),
            "needle pre-lowercased must match capitalized name"
        );
    }

    #[test]
    fn case_insensitive_name_match() {
        let scored = scored_candidates(vec![row("vim", None)], "VIM");
        assert_eq!(scored.len(), 1);
        assert!(scored[0].name_score.is_some());
    }

    #[test]
    fn desc_match_when_name_misses() {
        let scored = scored_candidates(vec![row("zzz", Some("fast editor"))], "edit");
        assert_eq!(scored.len(), 1);
        assert!(
            scored[0].name_score.is_none(),
            "name without needle should not name-match"
        );
        assert!(
            scored[0].desc_score.is_some(),
            "description containing needle should desc-match"
        );
    }

    #[test]
    fn no_match_yields_both_none() {
        let scored = scored_candidates(vec![row("zzz", Some("unrelated text"))], "edit");
        assert_eq!(scored.len(), 1);
        assert!(scored[0].name_score.is_none());
        assert!(scored[0].desc_score.is_none());
    }

    #[test]
    fn prefer_prefix_scores_at_least_substring() {
        let prefix_row = row("firefox", None);
        let substring_row = row("backfire", None);
        let scored = scored_candidates(vec![prefix_row, substring_row], "fire");
        assert_eq!(scored.len(), 2);
        let prefix_score = scored[0].name_score.expect("prefix matches");
        let substring_score = scored[1].name_score.expect("substring matches");
        assert!(
            prefix_score >= substring_score,
            "prefix match ({prefix_score}) should outrank substring match ({substring_score})"
        );
    }

    fn pkg(
        name: &str,
        source: PackageSource,
        last_update: Option<i64>,
        popularity: Option<f64>,
    ) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            source,
            description: None,
            version: "1".into(),
            repo: None,
            num_votes: None,
            popularity,
            installed: false,
            last_update,
            keywords: vec![],
        }
    }

    #[test]
    fn dedup_keeps_repo_over_aur() {
        let out = dedup_keep_repo(vec![
            pkg("foo", PackageSource::Aur, None, None),
            pkg("foo", PackageSource::Repo, None, None),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, PackageSource::Repo);
    }

    #[test]
    fn repo_recency_is_one_regardless_of_age() {
        let e = HashSet::new();
        let ancient = composite(
            &pkg("x", PackageSource::Repo, Some(0), None),
            "x",
            Some(10),
            None,
            None,
            &e,
            1e12,
        );
        let no_ts = composite(
            &pkg("x", PackageSource::Repo, None, None),
            "x",
            Some(10),
            None,
            None,
            &e,
            1e12,
        );
        assert_eq!(ancient, no_ts);
    }

    #[test]
    fn aur_recency_decays_with_age() {
        let e = HashSet::new();
        let recent = composite(
            &pkg("x", PackageSource::Aur, Some(9_999_000_000), None),
            "x",
            Some(10),
            None,
            None,
            &e,
            1e10,
        );
        let stale = composite(
            &pkg("x", PackageSource::Aur, Some(1_000_000_000), None),
            "x",
            Some(10),
            None,
            None,
            &e,
            1e10,
        );
        assert!(recent > stale);
    }

    #[test]
    fn installed_and_popularity_are_small_tiebreaks() {
        let e = HashSet::new();
        let base = composite(
            &pkg("a", PackageSource::Repo, None, None),
            "a",
            Some(20),
            None,
            None,
            &e,
            0.0,
        );
        let pop = composite(
            &pkg("b", PackageSource::Repo, None, Some(10.0)),
            "a",
            Some(20),
            None,
            None,
            &e,
            0.0,
        );
        let inst = HashSet::from(["a".to_string()]);
        let boosted = composite(
            &pkg("a", PackageSource::Repo, None, None),
            "a",
            Some(20),
            None,
            None,
            &inst,
            0.0,
        );
        assert!(boosted > pop && pop > base);
        assert!(
            composite(
                &pkg("c", PackageSource::Repo, None, None),
                "a",
                Some(100),
                None,
                None,
                &e,
                0.0
            ) > pop
        );
    }

    #[test]
    fn empty_query_returns_empty_and_limit_truncates() {
        let e = HashSet::new();
        assert!(score(Vec::new(), &SearchQuery::new(""), &e).is_empty());
        let rows = vec![
            pkg("alpha", PackageSource::Repo, None, None),
            pkg("beta", PackageSource::Repo, None, None),
        ];
        assert_eq!(
            score(
                rows,
                &SearchQuery {
                    text: "a".into(),
                    limit: 1
                },
                &e
            )
            .len(),
            1
        );
    }

    fn row_kw(name: &str, description: Option<&str>, keywords: Vec<String>) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            source: PackageSource::Aur,
            description: description.map(str::to_string),
            version: "1.0-1".to_string(),
            repo: Some("aur".to_string()),
            num_votes: None,
            popularity: None,
            installed: false,
            last_update: None,
            keywords,
        }
    }

    #[test]
    fn keyword_only_surfaces_amid_name_matches() {
        let e = HashSet::new();
        let candidates = vec![
            row("arm-none-eabi-gcc", None),
            row_kw("yay", Some("yet another yogurt"), vec!["arm".into()]),
        ];
        let results = score(candidates, &SearchQuery::new("arm"), &e);
        assert!(
            results.iter().any(|r| r.name == "yay"),
            "keyword-only hit must survive the name-match filter; got {:?}",
            results.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn name_outranks_equal_keyword() {
        let e = HashSet::new();
        let candidates = vec![row_kw("xyz", None, vec!["arm".into()]), row("arm", None)];
        let results = score(candidates, &SearchQuery::new("arm"), &e);
        assert_eq!(
            results[0].name, "arm",
            "exact name match must outrank an equal-strength keyword match"
        );
    }

    #[test]
    fn keyword_outranks_description_only() {
        let e = HashSet::new();
        let candidates = vec![
            row_kw("zzz", Some("unrelated stuff"), vec!["edit".into()]),
            row("qqq", Some("fast editor")),
        ];
        let results = score(candidates, &SearchQuery::new("edit"), &e);
        assert!(
            results.iter().any(|r| r.name == "zzz"),
            "keyword candidate must survive in the no-name-match regime"
        );
        let zzz_pos = results
            .iter()
            .position(|r| r.name == "zzz")
            .expect("zzz present");
        let qqq_pos = results
            .iter()
            .position(|r| r.name == "qqq")
            .expect("qqq present");
        assert!(
            zzz_pos < qqq_pos,
            "keyword match (W_KW=0.5) must outrank description-only (W_DESC=0.3)"
        );
    }

    #[test]
    fn long_named_keyword_match_present_not_first() {
        let e = HashSet::new();
        let candidates = vec![
            row_kw("010editor", None, vec!["sweetscape".into()]),
            row("sweetscape-bin", None),
        ];
        let results = score(candidates, &SearchQuery::new("sweetscape"), &e);
        assert!(
            results.iter().any(|r| r.name == "010editor"),
            "010editor must surface via its keyword; got {:?}",
            results.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
        );
        assert_ne!(
            results[0].name, "010editor",
            "keyword-only hit must not outrank a real name match"
        );
    }

    #[test]
    fn multiword_query_name_matches_when_each_token_is_a_subsequence() {
        let scored = scored_candidates(vec![row("google-chrome", None)], "google chr");
        assert_eq!(scored.len(), 1);
        assert!(
            scored[0].name_score.is_some(),
            "multi-word query must still name-match when every token is a subsequence of the name"
        );
    }

    #[test]
    fn multiword_query_requires_every_token_to_match() {
        let scored = scored_candidates(
            vec![row("python-google-auth-httplib2", None)],
            "google chr",
        );
        assert_eq!(scored.len(), 1);
        assert!(
            scored[0].name_score.is_none(),
            "AND semantics: a name missing one token's subsequence (no 'c' here) must yield None"
        );
    }

    #[test]
    fn multiword_query_ranks_chrome_above_python_google_auth() {
        let e = HashSet::new();
        let candidates = vec![
            row(
                "python-google-auth-httplib2",
                Some("Google Authentication Library: httplib2 transport"),
            ),
            row(
                "google-chrome",
                Some("The popular web browser by Google (Stable Channel)"),
            ),
        ];
        let results = score(candidates, &SearchQuery::new("google chr"), &e);
        assert!(
            !results.is_empty(),
            "results must be non-empty for a name-matching multi-word query"
        );
        assert_eq!(
            results[0].name, "google-chrome",
            "google-chrome must outrank python-google-auth-httplib2 for 'google chr'"
        );
    }
}
