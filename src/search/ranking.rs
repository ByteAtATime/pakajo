use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::search::SearchResult;

#[allow(dead_code)]
pub struct ScoredCandidate {
    pub result: SearchResult,
    pub name_score: Option<u16>,
    pub desc_score: Option<u16>,
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
            ScoredCandidate {
                result,
                name_score,
                desc_score,
            }
        })
        .collect()
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
}
