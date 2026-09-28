use crate::index::PackageIndex;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    ExactName = 0,
    ExactToken = 1,
    PrefixName = 2,
    PrefixToken = 3,
    Substring = 4,
    Keyword = 5,
    Fuzzy = 6,
}

pub(crate) struct Scored {
    pub tier: Tier,
    pub key: u64,
    pub pkg: u32,
}

pub(crate) const ALL_TIERS: &[Tier] = &[
    Tier::ExactName,
    Tier::ExactToken,
    Tier::PrefixName,
    Tier::PrefixToken,
    Tier::Substring,
    Tier::Keyword,
];

const FIRST_LETTER_SHIFT: u32 = 33;

pub(crate) fn rank_bits(name_len: u16, is_repo: bool, popularity: u16) -> u64 {
    (1 << FIRST_LETTER_SHIFT)
        | (name_len as u64) << 17
        | ((!is_repo) as u64) << 16
        | (u16::MAX - popularity) as u64
}

pub(crate) fn pack_sort_key(distance: u8, first_letter_match: bool, rank: u64) -> u64 {
    let not_first = (!first_letter_match) as u64;
    ((distance as u64) << 34)
        | (rank & !(1 << FIRST_LETTER_SHIFT))
        | (not_first << FIRST_LETTER_SHIFT)
}

pub(crate) fn tier_at(
    index: &PackageIndex,
    pi: usize,
    q: &str,
    allowed: &[Tier],
    qmask: u64,
    qbig: u64,
) -> Option<Tier> {
    let mut tokens = None;
    for &tier in allowed {
        match tier {
            Tier::ExactName => {
                if index.name(pi) == q {
                    return Some(tier);
                }
            }
            Tier::ExactToken | Tier::PrefixToken => {
                let hits = *tokens.get_or_insert_with(|| token_hits(index, pi, q));
                if (tier == Tier::ExactToken && hits.0) || (tier == Tier::PrefixToken && hits.1) {
                    return Some(tier);
                }
            }
            Tier::PrefixName => {
                if index.name(pi).starts_with(q) {
                    return Some(tier);
                }
            }
            Tier::Substring => {
                let r = index.row(pi);
                if r.name_len as usize >= q.len()
                    && (qmask & !r.name_mask) == 0
                    && (qbig & !r.name_bigrams) == 0
                    && index.name(pi).contains(q)
                {
                    return Some(tier);
                }
            }
            Tier::Keyword => {
                let r = index.row(pi);
                if index.max_kw_len(pi) as usize >= q.len()
                    && (qmask & !r.kw_mask) == 0
                    && (qbig & !r.kw_bigrams) == 0
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

fn token_hits(index: &PackageIndex, pi: usize, q: &str) -> (bool, bool) {
    let mut exact = false;
    let mut prefix = false;
    for k in 0..index.tokens_len(pi) {
        let token = index.token(pi, k);
        exact |= token == q;
        prefix |= token.starts_with(q);
    }
    (exact, prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{RawPkg, assemble, bigram_mask, byte_mask};

    const ALL_CONCRETE: &[Tier] = &[
        Tier::ExactName,
        Tier::ExactToken,
        Tier::PrefixName,
        Tier::PrefixToken,
        Tier::Substring,
        Tier::Keyword,
    ];

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
                byte_mask(b"google-chrome"),
                bigram_mask(b"google-chrome")
            ),
            Some(Tier::ExactName)
        );
        assert_eq!(
            tier_at(
                &index,
                0,
                "chrome",
                ALL_CONCRETE,
                byte_mask(b"chrome"),
                bigram_mask(b"chrome")
            ),
            Some(Tier::ExactToken)
        );
        assert_eq!(
            tier_at(
                &index,
                0,
                "chrom",
                ALL_CONCRETE,
                byte_mask(b"chrom"),
                bigram_mask(b"chrom")
            ),
            Some(Tier::PrefixToken)
        );
        assert_eq!(
            tier_at(
                &index,
                0,
                "xyz",
                ALL_CONCRETE,
                byte_mask(b"xyz"),
                bigram_mask(b"xyz")
            ),
            None
        );
        assert_eq!(
            tier_at(
                &index,
                0,
                "hrome",
                ALL_CONCRETE,
                byte_mask(b"hrome"),
                bigram_mask(b"hrome")
            ),
            Some(Tier::Substring)
        );
    }

    #[test]
    fn tier_at_keyword_respects_qmask() {
        let index = sample_index();
        assert_eq!(
            tier_at(
                &index,
                0,
                "brows",
                ALL_CONCRETE,
                byte_mask(b"brows"),
                bigram_mask(b"brows")
            ),
            Some(Tier::Keyword)
        );
        assert_eq!(
            tier_at(
                &index,
                0,
                "brows",
                ALL_CONCRETE,
                byte_mask(b"browsz"),
                bigram_mask(b"browsz")
            ),
            None
        );
    }
}
