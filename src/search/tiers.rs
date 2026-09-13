use std::cmp::Ordering;

use crate::search::index::PackageIndex;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    ExactName = 0,
    ExactToken = 1,
    PrefixName = 2,
    PrefixToken = 3,
    Substring = 4,
    Keyword = 5,
    Fuzzy = 6,
}

pub struct PkgView<'a> {
    pub name: &'a str,
    pub id: u32,
    pub popularity: u16,
    pub is_repo: bool,
}

pub struct Candidate<'a> {
    pub view: PkgView<'a>,
    pub tier: Tier,
    pub distance: u8,
    pub first_letter_match: bool,
}

pub fn candidate_ordering(a: &Candidate<'_>, b: &Candidate<'_>) -> Ordering {
    a.tier
        .cmp(&b.tier)
        .then_with(|| match (a.tier, b.tier) {
            (Tier::Fuzzy, Tier::Fuzzy) => a.distance.cmp(&b.distance),
            _ => std::cmp::Ordering::Equal,
        })
        .then_with(|| match (a.tier, b.tier) {
            (Tier::Fuzzy, Tier::Fuzzy) => b.first_letter_match.cmp(&a.first_letter_match),
            _ => std::cmp::Ordering::Equal,
        })
        .then_with(|| {
            a.view
                .name
                .chars()
                .count()
                .cmp(&b.view.name.chars().count())
        })
        .then_with(|| b.view.is_repo.cmp(&a.view.is_repo))
        .then_with(|| b.view.popularity.cmp(&a.view.popularity))
        .then_with(|| a.view.name.cmp(b.view.name))
        .then_with(|| a.view.id.cmp(&b.view.id))
}

pub fn tier_at(
    index: &PackageIndex,
    pi: usize,
    q: &str,
    allowed: &[Tier],
    qmask: u64,
) -> Option<Tier> {
    for &tier in allowed {
        match tier {
            Tier::ExactName => {
                if index.name(pi) == q {
                    return Some(tier);
                }
            }
            Tier::ExactToken => {
                if (0..index.tokens_len(pi)).any(|k| index.token(pi, k) == q) {
                    return Some(tier);
                }
            }
            Tier::PrefixName => {
                if index.name(pi).starts_with(q) {
                    return Some(tier);
                }
            }
            Tier::PrefixToken => {
                if (0..index.tokens_len(pi)).any(|k| index.token(pi, k).starts_with(q)) {
                    return Some(tier);
                }
            }
            Tier::Substring => {
                if (qmask & !index.row(pi).name_mask) == 0 && index.name(pi).contains(q) {
                    return Some(tier);
                }
            }
            Tier::Keyword => {
                if (qmask & !index.row(pi).kw_mask) == 0
                    && (0..index.kws_len(pi)).any(|k| index.keyword(pi, k).contains(q))
                {
                    return Some(tier);
                }
            }
            Tier::Fuzzy => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::{RawPkg, assemble, byte_mask};

    const ALL_CONCRETE: &[Tier] = &[
        Tier::ExactName,
        Tier::ExactToken,
        Tier::PrefixName,
        Tier::PrefixToken,
        Tier::Substring,
        Tier::Keyword,
    ];

    fn v(name: &str, id: u32, popularity: u16, is_repo: bool) -> PkgView<'_> {
        PkgView {
            name,
            id,
            popularity,
            is_repo,
        }
    }

    fn cand(view: PkgView<'_>, tier: Tier) -> Candidate<'_> {
        Candidate {
            view,
            tier,
            distance: 0,
            first_letter_match: false,
        }
    }

    fn sample_index() -> PackageIndex {
        let raws = vec![RawPkg {
            id: 1,
            name: "google-chrome".to_string(),
            tokens: vec!["google".to_string(), "chrome".to_string()],
            keywords: vec!["browser".to_string(), "web".to_string()],
            popularity: 100,
            is_repo: false,
        }];
        assemble(raws)
    }

    #[test]
    fn tier_at_matches_each_concrete_tier() {
        let index = sample_index();
        assert_eq!(
            tier_at(
                &index,
                0,
                "google-chrome",
                ALL_CONCRETE,
                byte_mask(b"google-chrome")
            ),
            Some(Tier::ExactName)
        );
        assert_eq!(
            tier_at(&index, 0, "chrome", ALL_CONCRETE, byte_mask(b"chrome")),
            Some(Tier::ExactToken)
        );
        assert_eq!(
            tier_at(&index, 0, "chrom", ALL_CONCRETE, byte_mask(b"chrom")),
            Some(Tier::PrefixToken)
        );
        assert_eq!(
            tier_at(&index, 0, "xyz", ALL_CONCRETE, byte_mask(b"xyz")),
            None
        );
        assert_eq!(
            tier_at(&index, 0, "hrome", ALL_CONCRETE, byte_mask(b"hrome")),
            Some(Tier::Substring)
        );
    }

    #[test]
    fn tier_at_keyword_respects_qmask() {
        let index = sample_index();
        assert_eq!(
            tier_at(&index, 0, "brows", ALL_CONCRETE, byte_mask(b"brows")),
            Some(Tier::Keyword)
        );
        assert_eq!(
            tier_at(&index, 0, "brows", ALL_CONCRETE, byte_mask(b"browsz")),
            None
        );
    }

    #[test]
    fn candidate_ordering_tier_dominates() {
        let ca = cand(v("alpha", 1, 0, false), Tier::ExactName);
        let cb = cand(v("exact", 2, 0, false), Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_shorter_name_first() {
        let ca = cand(v("vim", 1, 0, false), Tier::Substring);
        let cb = cand(v("vim-plugins", 2, 0, false), Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_fuzzy_lower_distance_first() {
        let ca = Candidate {
            view: v("alpha", 1, 0, false),
            tier: Tier::Fuzzy,
            distance: 1,
            first_letter_match: false,
        };
        let cb = Candidate {
            view: v("alpha", 2, 0, false),
            tier: Tier::Fuzzy,
            distance: 2,
            first_letter_match: false,
        };
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_fuzzy_first_letter_uses_matching_token() {
        let ca = Candidate {
            view: v("alpha-beta", 1, 0, false),
            tier: Tier::Fuzzy,
            distance: 1,
            first_letter_match: true,
        };
        let cb = Candidate {
            view: v("cedar-delta", 2, 0, false),
            tier: Tier::Fuzzy,
            distance: 1,
            first_letter_match: false,
        };
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
        assert_eq!(candidate_ordering(&cb, &ca), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_repo_before_non_repo() {
        let ca = cand(v("foo", 1, 0, true), Tier::Substring);
        let cb = cand(v("foo", 2, 0, false), Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);
    }

    #[test]
    fn candidate_ordering_higher_popularity_first() {
        let ca = cand(v("foo", 1, 10, false), Tier::Substring);
        let cb = cand(v("foo", 2, 90, false), Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Greater);
    }

    #[test]
    fn candidate_ordering_name_then_id_tiebreak() {
        let ca = cand(v("alpha", 1, 5, false), Tier::Substring);
        let cb = cand(v("zebra", 2, 5, false), Tier::Substring);
        assert_eq!(candidate_ordering(&ca, &cb), Ordering::Less);

        let cc = cand(v("same", 1, 5, false), Tier::Substring);
        let cd = cand(v("same", 2, 5, false), Tier::Substring);
        assert_eq!(candidate_ordering(&cc, &cd), Ordering::Less);
    }
}
