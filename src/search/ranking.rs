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
    let needle_lc = needle.to_lowercase();
    let mut nbuf: Vec<char> = Vec::new();
    let needle_utf32 = Utf32Str::new(&needle_lc, &mut nbuf);
    let mut hbuf: Vec<char> = Vec::new();
    candidates
        .into_iter()
        .map(|result| {
            hbuf.clear();
            let name = Utf32Str::new(&result.name, &mut hbuf);
            let name_score = matcher.fuzzy_match(name, needle_utf32);
            hbuf.clear();
            let desc_score = result.description.as_ref().and_then(|d| {
                let d = Utf32Str::new(d, &mut hbuf);
                matcher.fuzzy_match(d, needle_utf32)
            });
            let keyword_score = if result.keywords.is_empty() {
                None
            } else {
                let mut best: Option<u16> = None;
                for kw in &result.keywords {
                    hbuf.clear();
                    let kw_s = Utf32Str::new(kw, &mut hbuf);
                    if let Some(sc) = matcher.fuzzy_match(kw_s, needle_utf32) {
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
const W_POP: f64 = 0.2;
const W_REPO: f64 = 0.1;
const W_RECENCY: f64 = 0.1;
const W_INSTALLED: f64 = 0.5;
const K_NAME: f64 = 100.0;
const K_DESC: f64 = 100.0;
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
            .filter(|c| c.name_score.is_some())
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
    installed: &HashSet<String>,
    now: f64,
) -> f64 {
    let needle_len = needle.chars().count() as f64;
    let name_len = r.name.chars().count() as f64;
    let precision = (needle_len / name_len).min(1.0);

    let norm_name = name_score
        .map(|s| saturate(s as f64, K_NAME))
        .unwrap_or(0.0);
    let norm_desc = desc_score
        .map(|s| saturate(s as f64, K_DESC))
        .unwrap_or(0.0);
    let norm_pop = r.popularity.map(|p| saturate(p, K_POP)).unwrap_or(0.0);

    let text = norm_name * precision + W_DESC * norm_desc * (1.0 - norm_name);

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
            &e,
            1e12,
        );
        let no_ts = composite(
            &pkg("x", PackageSource::Repo, None, None),
            "x",
            Some(10),
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
            &e,
            1e10,
        );
        let stale = composite(
            &pkg("x", PackageSource::Aur, Some(1_000_000_000), None),
            "x",
            Some(10),
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
            &e,
            0.0,
        );
        let pop = composite(
            &pkg("b", PackageSource::Repo, None, Some(10.0)),
            "a",
            Some(20),
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
}
