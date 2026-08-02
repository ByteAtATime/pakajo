use std::cmp::Ordering;

use crate::search::index::IndexedPackage;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    ExactName = 0,
    ExactToken = 1,
    PrefixName = 2,
    PrefixToken = 3,
    Substring = 4,
    Keyword = 5,
    Typo = 6,
}

pub fn exact_name(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.name == q
}

pub fn exact_token(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.tokens.iter().any(|t| t == q)
}

pub fn prefix_name(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.name.starts_with(q)
}

pub fn prefix_token(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.tokens.iter().any(|t| t.starts_with(q))
}

pub fn substring(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.name.contains(q)
}

pub fn keyword(pkg: &IndexedPackage, q: &str) -> bool {
    pkg.keywords.iter().any(|k| k.contains(q))
}

pub fn best_concrete_tier_in(
    pkg: &IndexedPackage,
    q: &str,
    allowed: &[Tier],
    qmask: u64,
) -> Option<Tier> {
    if allowed.contains(&Tier::ExactName) && exact_name(pkg, q) {
        return Some(Tier::ExactName);
    }
    if allowed.contains(&Tier::ExactToken) && exact_token(pkg, q) {
        return Some(Tier::ExactToken);
    }
    if allowed.contains(&Tier::PrefixName) && prefix_name(pkg, q) {
        return Some(Tier::PrefixName);
    }
    if allowed.contains(&Tier::PrefixToken) && prefix_token(pkg, q) {
        return Some(Tier::PrefixToken);
    }
    if allowed.contains(&Tier::Substring)
        && (qmask & !pkg.name_mask) == 0
        && substring(pkg, q)
    {
        return Some(Tier::Substring);
    }
    if allowed.contains(&Tier::Keyword)
        && (qmask & !pkg.kw_mask) == 0
        && keyword(pkg, q)
    {
        return Some(Tier::Keyword);
    }
    None
}

pub struct Candidate<'a> {
    pub pkg: &'a IndexedPackage,
    pub tier: Tier,
    pub distance: u8,
}

pub fn candidate_ordering(a: &Candidate<'_>, b: &Candidate<'_>) -> Ordering {
    a.tier
        .cmp(&b.tier)
        .then_with(|| match (a.tier, b.tier) {
            (Tier::Typo, Tier::Typo) => a.distance.cmp(&b.distance),
            _ => std::cmp::Ordering::Equal,
        })
        .then_with(|| a.pkg.name.chars().count().cmp(&b.pkg.name.chars().count()))
        .then_with(|| b.pkg.is_repo.cmp(&a.pkg.is_repo))
        .then_with(|| b.pkg.popularity.cmp(&a.pkg.popularity))
        .then_with(|| a.pkg.name.cmp(&b.pkg.name))
        .then_with(|| a.pkg.id.cmp(&b.pkg.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::byte_mask;

    fn mk(
        id: u32,
        name: &str,
        tokens: &[&str],
        keywords: &[&str],
        popularity: u16,
        is_repo: bool,
    ) -> IndexedPackage {
        let tokens: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
        let keywords: Vec<String> = keywords.iter().map(|k| k.to_string()).collect();
        let name_mask = byte_mask(name.as_bytes());
        let kw_mask = keywords
            .iter()
            .map(|k| byte_mask(k.as_bytes()))
            .fold(0u64, |acc, m| acc | m);
        IndexedPackage {
            id,
            name: name.to_string(),
            tokens,
            keywords,
            popularity,
            is_repo,
            name_mask,
            kw_mask,
        }
    }

    fn chrome() -> IndexedPackage {
        mk(1, "google-chrome", &["google", "chrome"], &["browser", "web"], 100, false)
    }

    fn cand(pkg: &IndexedPackage, tier: Tier) -> Candidate<'_> {
        Candidate { pkg, tier, distance: 0 }
    }

    #[test]
    fn tier_ordinals_are_strictly_increasing() {
        assert!(Tier::ExactName < Tier::ExactToken);
        assert!(Tier::ExactToken < Tier::PrefixName);
        assert!(Tier::PrefixName < Tier::PrefixToken);
        assert!(Tier::PrefixToken < Tier::Substring);
        assert!(Tier::Substring < Tier::Keyword);
        assert!(Tier::Keyword < Tier::Typo);
    }

    #[test]
    fn predicates_match_and_miss_on_chrome_pkg() {
        let pkg = chrome();
        assert!(exact_name(&pkg, "google-chrome"));
        assert!(!exact_name(&pkg, "chrome"));
        assert!(exact_token(&pkg, "chrome"));
        assert!(!exact_token(&pkg, "chromium"));
        assert!(prefix_name(&pkg, "google"));
        assert!(!prefix_name(&pkg, "chrome"));
        assert!(prefix_token(&pkg, "chrom"));
        assert!(!prefix_token(&pkg, "zilla"));
        assert!(substring(&pkg, "chrome"));
        assert!(!substring(&pkg, "zilla"));
        assert!(keyword(&pkg, "brows"));
        assert!(!keyword(&pkg, "editor"));
    }

    #[test]
    fn best_concrete_tier_chooses_lowest_ordinal_match() {
        const ALL_CONCRETE: &[Tier] = &[
            Tier::ExactName,
            Tier::ExactToken,
            Tier::PrefixName,
            Tier::PrefixToken,
            Tier::Substring,
            Tier::Keyword,
        ];
        let pkg = chrome();
        assert_eq!(
            best_concrete_tier_in(&pkg, "google-chrome", ALL_CONCRETE, byte_mask(b"google-chrome")),
            Some(Tier::ExactName),
        );
        assert_eq!(
            best_concrete_tier_in(&pkg, "chrome", ALL_CONCRETE, byte_mask(b"chrome")),
            Some(Tier::ExactToken),
        );
        assert_eq!(
            best_concrete_tier_in(&pkg, "chrom", ALL_CONCRETE, byte_mask(b"chrom")),
            Some(Tier::PrefixToken),
        );
        assert_eq!(
            best_concrete_tier_in(&pkg, "xyz", ALL_CONCRETE, byte_mask(b"xyz")),
            None,
        );
    }

    #[test]
    fn candidate_ordering_tier_dominates() {
        let a = mk(1, "alpha", &[], &[], 0, false);
        let b = mk(2, "exact", &[], &[], 0, false);
        let ca = cand(&a, Tier::ExactName);
        let cb = cand(&b, Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_shorter_name_first() {
        let a = mk(1, "vim", &[], &[], 0, false);
        let b = mk(2, "vim-plugins", &[], &[], 0, false);
        let ca = cand(&a, Tier::Substring);
        let cb = cand(&b, Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_typo_lower_distance_first() {
        let a = mk(1, "alpha", &[], &[], 0, false);
        let b = mk(2, "alpha", &[], &[], 0, false);
        let ca = Candidate { pkg: &a, tier: Tier::Typo, distance: 1 };
        let cb = Candidate { pkg: &b, tier: Tier::Typo, distance: 2 };
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_repo_before_non_repo() {
        let a = mk(1, "foo", &[], &[], 0, true);
        let b = mk(2, "foo", &[], &[], 0, false);
        let ca = cand(&a, Tier::Substring);
        let cb = cand(&b, Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
    }

    #[test]
    fn candidate_ordering_higher_popularity_first() {
        let a = mk(1, "foo", &[], &[], 10, false);
        let b = mk(2, "foo", &[], &[], 90, false);
        let ca = cand(&a, Tier::Substring);
        let cb = cand(&b, Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_name_then_id_tiebreak() {
        let a = mk(1, "alpha", &[], &[], 5, false);
        let b = mk(2, "zebra", &[], &[], 5, false);
        let ca = cand(&a, Tier::Substring);
        let cb = cand(&b, Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);

        let c = mk(1, "same", &[], &[], 5, false);
        let d = mk(2, "same", &[], &[], 5, false);
        let cc = cand(&c, Tier::Substring);
        let cd = cand(&d, Tier::Substring);
        assert_eq!(candidate_ordering(&cc, &cd), Ordering::Less);
    }
}
