use crate::search::index::IndexedPackage;
use crate::search::tiers::Tier;

const MAX_EDIT_DISTANCE: usize = 2;

pub fn typo_tier(pkg: &IndexedPackage, q: &str) -> Option<Tier> {
    if q.is_empty() {
        return None;
    }
    if edit_distance_at_most(pkg.name.as_str(), q, MAX_EDIT_DISTANCE) {
        return Some(Tier::Typo);
    }
    if pkg.tokens.iter().any(|t| edit_distance_at_most(t.as_str(), q, MAX_EDIT_DISTANCE)) {
        return Some(Tier::Typo);
    }
    None
}

fn edit_distance_at_most(a: &str, b: &str, max: usize) -> bool {
    if a == b {
        return true;
    }
    if a.is_ascii() && b.is_ascii() && a.len().abs_diff(b.len()) > max {
        return false;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > max {
        return false;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let del = prev[j] + 1;
            let ins = cur[j - 1] + 1;
            let sub = prev[j - 1] + cost;
            cur[j] = del.min(ins).min(sub);
            if cur[j] < row_min {
                row_min = cur[j];
            }
        }
        if row_min > max {
            return false;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m] <= max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::IndexedPackage;

    fn mk(
        id: u32,
        name: &str,
        tokens: &[&str],
        keywords: &[&str],
        popularity: u16,
        is_repo: bool,
    ) -> IndexedPackage {
        IndexedPackage {
            id,
            name: name.to_string(),
            tokens: tokens.iter().map(|t| t.to_string()).collect(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            popularity,
            is_repo,
        }
    }

    #[test]
    fn edit_distance_exact_is_zero() {
        assert!(edit_distance_at_most("chrome", "chrome", 2));
    }

    #[test]
    fn edit_distance_transposition_caught() {
        assert!(edit_distance_at_most("chrome", "chroem", 2));
    }

    #[test]
    fn edit_distance_single_sub() {
        assert!(edit_distance_at_most("chrome", "chromm", 2));
    }

    #[test]
    fn edit_distance_too_far() {
        assert!(!edit_distance_at_most("chrome", "chromium", 2));
    }

    #[test]
    fn edit_distance_unrelated() {
        assert!(!edit_distance_at_most("abc", "xyz", 2));
    }

    #[test]
    fn edit_distance_length_cutoff() {
        assert!(!edit_distance_at_most("a", "abcd", 2));
    }

    #[test]
    fn typo_tier_via_token_transposition() {
        let pkg = mk(1, "google-chrome", &["google", "chrome"], &[], 0, false);
        assert_eq!(typo_tier(&pkg, "chroem"), Some(Tier::Typo));
    }

    #[test]
    fn typo_tier_via_name() {
        let pkg = mk(1, "vim", &[], &[], 0, false);
        assert_eq!(typo_tier(&pkg, "vom"), Some(Tier::Typo));
    }

    #[test]
    fn typo_tier_none() {
        let pkg = mk(1, "google-chrome", &["google", "chrome"], &[], 0, false);
        assert_eq!(typo_tier(&pkg, "xyz123"), None);
    }

    #[test]
    fn typo_tier_empty_query() {
        let pkg = mk(1, "google-chrome", &["google", "chrome"], &[], 0, false);
        assert_eq!(typo_tier(&pkg, ""), None);
    }
}
