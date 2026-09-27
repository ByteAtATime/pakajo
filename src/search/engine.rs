use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::search::SearchFilter;
use crate::search::fuzzy::{FuzzyMatcher, MAX_EDIT_DISTANCE, at_most_two_missing};
use crate::search::index::{CHAR_BITS, PackageIndex, bigram_mask, byte_mask, needs_rebuild};
use crate::search::query::{ParsedQuery, parse_query};
use crate::search::tiers::{Scored, Tier, pack_sort_key, tier_at, tier_of_key};

const RESULT_LIMIT: usize = 30;
const FUZZY_GATE: usize = 5;
const MIN_BITMAP_QUERY_CHARS: u32 = 3;
const MAX_BITMAP_QUERY_CHARS: u32 = 12;

const SHORT_TIERS: &[Tier] = &[Tier::ExactName, Tier::ExactToken, Tier::PrefixName];
const CHEAP_TIERS_ALL: &[Tier] = &[
    Tier::ExactName,
    Tier::ExactToken,
    Tier::PrefixName,
    Tier::PrefixToken,
];
const EXPENSIVE_TIERS: &[Tier] = &[Tier::Substring, Tier::Keyword];

#[cfg(test)]
pub fn search_index(index: &PackageIndex, text: &str) -> Vec<u32> {
    search_index_tiered(index, text, SearchFilter::All, &HashSet::new())
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

pub fn search_index_tiered(
    index: &PackageIndex,
    text: &str,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<(u32, Tier)> {
    let Some(pq) = parse_query(text) else {
        return Vec::new();
    };
    let q = pq.text();
    match &pq {
        ParsedQuery::Quoted(_) => quoted_pairs(index, q, filter, installed),
        ParsedQuery::Short(_) => to_sorted_pairs(
            index,
            gather_cheap_candidates(index, q, SHORT_TIERS, filter, installed),
        ),
        ParsedQuery::Normal(_) => {
            if q.split_whitespace().count() > 1 {
                let cands = multi_term_candidates(index, q, filter, installed);
                if !cands.is_empty() {
                    return to_sorted_pairs(index, cands);
                }
            }
            let cheap = gather_cheap_candidates(index, q, CHEAP_TIERS_ALL, filter, installed);
            if cheap.len() >= RESULT_LIMIT {
                return to_sorted_pairs(index, cheap);
            }
            let cands = SCAN_SEEN.with(|seen_cell| {
                SCAN_PLACED.with(|placed_cell| {
                    let mut seen = seen_cell.borrow_mut();
                    let mut placed = placed_cell.borrow_mut();
                    if cheap.len() >= FUZZY_GATE {
                        expensive_only_pass(index, q, cheap, &mut seen, filter, installed)
                    } else {
                        fused_expensive_fuzzy_pass(
                            index,
                            q,
                            cheap,
                            &mut seen,
                            &mut placed,
                            filter,
                            installed,
                        )
                    }
                })
            });
            to_sorted_pairs(index, cands)
        }
    }
}

pub struct SearchEngine {
    sqlite_path: PathBuf,
    index: RwLock<Arc<PackageIndex>>,
}

impl SearchEngine {
    pub fn new(sqlite_path: PathBuf) -> anyhow::Result<Self> {
        let index = PackageIndex::load_or_build(&sqlite_path)?;
        Ok(Self {
            sqlite_path,
            index: RwLock::new(Arc::new(index)),
        })
    }

    #[cfg(test)]
    pub fn search(&self, text: &str) -> Vec<u32> {
        let snapshot = self.index.read().expect("index lock poisoned").clone();
        search_index(&snapshot, text)
    }

    pub fn search_tiered(
        &self,
        text: &str,
        filter: SearchFilter,
        installed: &HashSet<String>,
    ) -> Vec<(u32, Tier)> {
        let snapshot = self.index.read().expect("index lock poisoned").clone();
        search_index_tiered(&snapshot, text, filter, installed)
    }

    pub fn ensure_fresh(&self) -> anyhow::Result<()> {
        if !needs_rebuild(&self.sqlite_path) {
            return Ok(());
        }
        let mut guard = self.index.write().expect("index lock poisoned");
        if !needs_rebuild(&self.sqlite_path) {
            return Ok(());
        }
        let rebuilt = PackageIndex::load_or_build(&self.sqlite_path)?;
        *guard = Arc::new(rebuilt);
        Ok(())
    }
}

fn scored_push(
    out: &mut Vec<Scored>,
    index: &PackageIndex,
    pkg: usize,
    tier: Tier,
    distance: u8,
    first_letter_match: bool,
) {
    out.push(Scored {
        key: pack_sort_key(tier, distance, first_letter_match, index.rank_word(pkg)),
        pkg: pkg as u32,
    });
}

fn scored_ordering(index: &PackageIndex, a: &Scored, b: &Scored) -> Ordering {
    a.key
        .cmp(&b.key)
        .then_with(|| index.name(a.pkg as usize).cmp(index.name(b.pkg as usize)))
        .then_with(|| {
            index
                .row(a.pkg as usize)
                .id
                .cmp(&index.row(b.pkg as usize).id)
        })
}

thread_local! {
    static GATHER: RefCell<EpochSet> = const { RefCell::new(EpochSet::new()) };
    static SCAN_SEEN: RefCell<EpochSet> = const { RefCell::new(EpochSet::new()) };
    static SCAN_PLACED: RefCell<EpochSet> = const { RefCell::new(EpochSet::new()) };
    static PREFILTER: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

struct EpochSet {
    slots: Vec<u32>,
    epoch: u32,
}

impl EpochSet {
    const fn new() -> Self {
        Self {
            slots: Vec::new(),
            epoch: 0,
        }
    }

    fn reset(&mut self, len: usize) {
        if self.slots.len() < len {
            self.slots.resize(len, 0);
        }
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
        }
        if self.epoch == 1 {
            self.slots.fill(0);
        }
    }

    fn insert(&mut self, pkg: usize) {
        self.slots[pkg] = self.epoch;
    }

    fn contains(&self, pkg: usize) -> bool {
        self.slots[pkg] == self.epoch
    }
}

fn gather_cheap_candidates(
    index: &PackageIndex,
    q: &str,
    allowed: &[Tier],
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<Scored> {
    let q_bytes = q.as_bytes();
    let only_all = filter == SearchFilter::All;
    GATHER.with(|state| {
        let mut marks = state.borrow_mut();
        marks.reset(index.len());
        let mut out: Vec<Scored> = Vec::new();
        for &tier in allowed {
            let (from_tokens, range) = match tier {
                Tier::ExactName => (false, index.exact_name_range(q_bytes)),
                Tier::ExactToken => (true, index.exact_token_range(q_bytes)),
                Tier::PrefixName => (false, index.prefix_name_range(q_bytes)),
                Tier::PrefixToken => (true, index.prefix_token_range(q_bytes)),
                _ => continue,
            };
            let stop_in_range = only_all && matches!(tier, Tier::ExactToken);
            let postings = range.map(|i| {
                if from_tokens {
                    index.tokens_sorted[i].1 as usize
                } else {
                    index.names_sorted[i] as usize
                }
            });
            for pkg in postings {
                if marks.contains(pkg) {
                    continue;
                }
                marks.insert(pkg);
                if filter.matches(index.view(pkg), installed) {
                    scored_push(&mut out, index, pkg, tier, 0, false);
                }
                if stop_in_range && out.len() >= RESULT_LIMIT {
                    break;
                }
            }
            if only_all && out.len() >= RESULT_LIMIT {
                break;
            }
        }
        out
    })
}

fn expensive_only_pass(
    index: &PackageIndex,
    q: &str,
    mut cands: Vec<Scored>,
    seen: &mut EpochSet,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<Scored> {
    let qmask = byte_mask(q.as_bytes());
    let qbig = bigram_mask(q.as_bytes());
    seen.reset(index.len());
    for c in &cands {
        seen.insert(c.pkg as usize);
    }
    for i in 0..index.len() {
        if seen.contains(i) {
            continue;
        }
        if filter.matches(index.view(i), installed)
            && let Some(tier) = tier_at(index, i, q, EXPENSIVE_TIERS, qmask, qbig)
        {
            scored_push(&mut cands, index, i, tier, 0, false);
        }
    }
    cands
}

fn seed_words(index: &PackageIndex, qmask: u64, out: &mut Vec<u64>) {
    let chars: Vec<usize> = (0..CHAR_BITS).filter(|b| qmask >> b & 1 == 1).collect();
    out.clear();
    out.resize(index.row_words(), 0);
    for (w, slot) in out.iter_mut().enumerate() {
        let mut name_all = u64::MAX;
        let mut kw_all = u64::MAX;
        for &b in &chars {
            name_all &= index.name_char_word(w, b);
            kw_all &= index.kw_char_word(w, b);
        }
        *slot = name_all | kw_all;
    }
    mask_tail(index, out);
}

fn seed_rows(
    index: &PackageIndex,
    term: &str,
    qmask: u64,
    qbig: u64,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<(usize, Tier)> {
    PREFILTER.with(|cell| {
        let mut words = cell.borrow_mut();
        seed_words(index, qmask, &mut words);
        let mut out: Vec<(usize, Tier)> = Vec::new();
        for_each_row(&words, |pkg| {
            if let Some(tier) = term_tier_at(index, pkg, term, qmask, qbig, filter, installed) {
                out.push((pkg, tier));
            }
        });
        out
    })
}

fn term_tier_at(
    index: &PackageIndex,
    pkg: usize,
    term: &str,
    qmask: u64,
    qbig: u64,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Option<Tier> {
    if !filter.matches(index.view(pkg), installed) {
        return None;
    }
    let name = index.name(pkg);
    if name == term {
        return Some(Tier::ExactName);
    }
    let mut prefix_token = false;
    for k in 0..index.tokens_len(pkg) {
        let token = index.token(pkg, k);
        if token == term {
            return Some(Tier::ExactToken);
        }
        prefix_token |= token.starts_with(term);
    }
    if name.starts_with(term) {
        return Some(Tier::PrefixName);
    }
    if prefix_token {
        return Some(Tier::PrefixToken);
    }
    let r = index.row(pkg);
    if (qmask & !r.name_mask) == 0 && (qbig & !r.name_bigrams) == 0 && name.contains(term) {
        return Some(Tier::Substring);
    }
    if (qmask & !r.kw_mask) == 0
        && (qbig & !r.kw_bigrams) == 0
        && (0..index.kws_len(pkg)).any(|k| index.keyword(pkg, k).contains(term))
    {
        return Some(Tier::Keyword);
    }
    None
}

fn term_matches_any(
    index: &PackageIndex,
    term: &str,
    qmask: u64,
    qbig: u64,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> bool {
    !seed_rows(index, term, qmask, qbig, filter, installed).is_empty()
}

fn normalized_name_tiers(
    index: &PackageIndex,
    q: &str,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<(usize, Tier)> {
    let nq: String = q.chars().filter(|c| c.is_alphanumeric()).collect();
    let mut out: Vec<(usize, Tier)> = Vec::new();
    if nq.is_empty() {
        return out;
    }
    let nqmask = byte_mask(nq.as_bytes());
    for pkg in 0..index.len() {
        let r = index.row(pkg);
        if (nqmask & !r.name_mask) != 0 || !filter.matches(index.view(pkg), installed) {
            continue;
        }
        let nn = index.norm_name(pkg);
        let tier = if nn == nq {
            Tier::ExactName
        } else if nn.starts_with(&nq) {
            Tier::PrefixName
        } else if nn.contains(&nq) {
            Tier::Substring
        } else {
            continue;
        };
        out.push((pkg, tier));
    }
    out
}

fn multi_term_candidates(
    index: &PackageIndex,
    q: &str,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<Scored> {
    let mut survivors: Vec<(usize, Tier)> = Vec::new();
    for term in q.split_whitespace() {
        let qmask = byte_mask(term.as_bytes());
        let qbig = bigram_mask(term.as_bytes());
        if survivors.is_empty() {
            survivors = seed_rows(index, term, qmask, qbig, filter, installed);
            continue;
        }
        let mut kept: Vec<(usize, Tier)> = Vec::new();
        for (pkg, tier) in survivors.iter().copied() {
            if let Some(other) = term_tier_at(index, pkg, term, qmask, qbig, filter, installed) {
                kept.push((pkg, tier.max(other)));
            }
        }
        if kept.is_empty() {
            if term_matches_any(index, term, qmask, qbig, filter, installed) {
                survivors.clear();
            }
            continue;
        }
        survivors = kept;
    }
    if survivors.is_empty() {
        return Vec::new();
    }
    for (pkg, tier) in normalized_name_tiers(index, q, filter, installed) {
        match survivors.binary_search_by_key(&pkg, |&(p, _)| p) {
            Ok(pos) => survivors[pos].1 = survivors[pos].1.min(tier),
            Err(pos) => survivors.insert(pos, (pkg, tier)),
        }
    }
    let mut out = Vec::with_capacity(survivors.len());
    for (pkg, tier) in survivors {
        scored_push(&mut out, index, pkg, tier, 0, false);
    }
    out
}

fn fuzzy_score_from_seed(
    matcher: &mut FuzzyMatcher,
    index: &PackageIndex,
    pi: usize,
    seed_distance: u8,
    seed_first_letter: bool,
    q_first: Option<char>,
) -> (u8, bool) {
    let mut distance = seed_distance;
    let mut first_letter_match = seed_first_letter;
    for k in 0..index.tokens_len(pi) {
        let t = index.token(pi, k);
        let tmask = byte_mask(t.as_bytes());
        if let Some(d) = matcher.within_distance(t.as_bytes(), tmask, MAX_EDIT_DISTANCE) {
            distance = distance.min(d as u8);
            if !first_letter_match && t.chars().next() == q_first {
                first_letter_match = true;
            }
        }
    }
    (distance, first_letter_match)
}

#[derive(Clone, Copy)]
struct FusedCtx<'a> {
    index: &'a PackageIndex,
    q: &'a str,
    qmask: u64,
    qbig: u64,
    q_ascii: bool,
    q_first: Option<char>,
    filter: SearchFilter,
    installed: &'a HashSet<String>,
}

fn drop_subsets(n: usize) -> Vec<u64> {
    let mut out = vec![0u64];
    for d in 0..n {
        out.push(1u64 << d);
    }
    for a in 0..n {
        for b in a + 1..n {
            out.push((1u64 << a) | (1u64 << b));
        }
    }
    out
}

fn prefilter_words(index: &PackageIndex, qmask: u64, out: &mut Vec<u64>) {
    let words = index.row_words();
    out.clear();
    out.resize(words, 0);
    let chars: Vec<usize> = (0..CHAR_BITS).filter(|b| qmask >> b & 1 == 1).collect();
    let dropped = drop_subsets(chars.len());
    for (w, slot) in out.iter_mut().enumerate() {
        let mut name_union = 0u64;
        for d in &dropped {
            let mut acc = u64::MAX;
            for (ci, &b) in chars.iter().enumerate() {
                if d >> ci & 1 == 0 {
                    acc &= index.name_char_word(w, b);
                }
            }
            name_union |= acc;
        }
        let mut kw_all = u64::MAX;
        for &b in &chars {
            kw_all &= index.kw_char_word(w, b);
        }
        *slot = name_union | kw_all;
    }
    mask_tail(index, out);
}

fn full_words(index: &PackageIndex, out: &mut Vec<u64>) {
    out.clear();
    out.resize(index.row_words(), u64::MAX);
    mask_tail(index, out);
}

fn mask_tail(index: &PackageIndex, words: &mut [u64]) {
    let tail = index.len() % 64;
    if tail != 0
        && let Some(last) = words.last_mut()
    {
        *last &= (1u64 << tail) - 1;
    }
}

fn fused_expensive_row(
    ctx: &FusedCtx,
    i: usize,
    matcher: &mut FuzzyMatcher,
    cands: &mut Vec<Scored>,
    fuzzy_buf: &mut Vec<Scored>,
    placed: &mut EpochSet,
) {
    let FusedCtx {
        index,
        q,
        qmask,
        qbig,
        q_ascii,
        q_first,
        filter,
        installed,
    } = *ctx;
    let r = index.row(i);
    let name_missing = qmask & !r.name_mask;
    let name_tier_arm = r.name_len as usize >= q.len() && (qbig & !r.name_bigrams) == 0;
    let kw_tier_arm = index.max_kw_len(i) as usize >= q.len() && (qbig & !r.kw_bigrams) == 0;
    if (name_tier_arm || kw_tier_arm)
        && let Some(tier) = tier_at(index, i, q, EXPENSIVE_TIERS, qmask, qbig)
    {
        if filter.matches(index.view(i), installed) {
            scored_push(cands, index, i, tier, 0, false);
        }
        placed.insert(i);
        return;
    }
    let name_extra = r.name_mask & !qmask;
    if q_ascii {
        if !at_most_two_missing(name_missing) || !at_most_two_missing(name_extra) {
            return;
        }
    } else if name_missing.count_ones() as usize + name_extra.count_ones() as usize
        > MAX_EDIT_DISTANCE
    {
        return;
    }
    let name_len_ok =
        !(q_ascii && r.ascii_name) || (r.name_len as usize).abs_diff(q.len()) <= MAX_EDIT_DISTANCE;
    if !name_len_ok {
        return;
    }
    let Some(name_d) =
        matcher.within_distance(index.name(i).as_bytes(), r.name_mask, MAX_EDIT_DISTANCE)
    else {
        return;
    };
    if !filter.matches(index.view(i), installed) {
        placed.insert(i);
        return;
    }
    let seed_first_letter = index.name(i).chars().next() == q_first;
    let (distance, first_letter_match) =
        fuzzy_score_from_seed(matcher, index, i, name_d as u8, seed_first_letter, q_first);
    scored_push(
        fuzzy_buf,
        index,
        i,
        Tier::Fuzzy,
        distance,
        first_letter_match,
    );
    placed.insert(i);
}

fn fused_expensive_fuzzy_pass(
    index: &PackageIndex,
    q: &str,
    mut cands: Vec<Scored>,
    seen: &mut EpochSet,
    placed: &mut EpochSet,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<Scored> {
    let qmask = byte_mask(q.as_bytes());
    let q_ascii = q.is_ascii();
    let ctx = FusedCtx {
        index,
        q,
        qmask,
        qbig: bigram_mask(q.as_bytes()),
        q_ascii,
        q_first: q.chars().next(),
        filter,
        installed,
    };
    let mut matcher = FuzzyMatcher::new(q.as_bytes());
    let mut fuzzy_buf: Vec<Scored> = Vec::new();
    seen.reset(index.len());
    placed.reset(index.len());
    for c in &cands {
        seen.insert(c.pkg as usize);
        placed.insert(c.pkg as usize);
    }

    PREFILTER.with(|cell| {
        let mut words = cell.borrow_mut();
        let query_chars = qmask.count_ones();
        if (MIN_BITMAP_QUERY_CHARS..=MAX_BITMAP_QUERY_CHARS).contains(&query_chars) {
            prefilter_words(index, qmask, &mut words);
        } else {
            full_words(index, &mut words);
        }
        for_each_row(&words, |i| {
            if seen.contains(i) {
                return;
            }
            fused_expensive_row(&ctx, i, &mut matcher, &mut cands, &mut fuzzy_buf, placed);
        });
    });

    for slot in 0..index.token_scan_ids.len() {
        let token_mask = index.token_scan_masks[slot];
        let token_missing = qmask & !token_mask;
        let token_extra = token_mask & !qmask;
        if q_ascii {
            if !at_most_two_missing(token_missing) || !at_most_two_missing(token_extra) {
                continue;
            }
        } else if token_missing.count_ones() as usize + token_extra.count_ones() as usize
            > MAX_EDIT_DISTANCE
        {
            continue;
        }
        let tid = index.token_scan_ids[slot] as usize;
        let token = index.token_str(tid);
        if q_ascii && token.len().abs_diff(q.len()) > MAX_EDIT_DISTANCE {
            continue;
        }
        let Some(tok_d) = matcher.within_distance(token.as_bytes(), token_mask, MAX_EDIT_DISTANCE)
        else {
            continue;
        };
        for j in index.exact_token_range(token.as_bytes()) {
            let pkg_idx = index.tokens_sorted[j].1 as usize;
            if placed.contains(pkg_idx) {
                continue;
            }
            if !filter.matches(index.view(pkg_idx), installed) {
                placed.insert(pkg_idx);
                continue;
            }
            let seed_first_letter = token.chars().next() == ctx.q_first;
            let (distance, first_letter_match) = fuzzy_score_from_seed(
                &mut matcher,
                index,
                pkg_idx,
                tok_d as u8,
                seed_first_letter,
                ctx.q_first,
            );
            scored_push(
                &mut fuzzy_buf,
                index,
                pkg_idx,
                Tier::Fuzzy,
                distance,
                first_letter_match,
            );
            placed.insert(pkg_idx);
        }
    }

    if cands.len() < FUZZY_GATE {
        cands.append(&mut fuzzy_buf);
    }
    cands
}

fn bigram_words(index: &PackageIndex, qbig: u64, out: &mut Vec<u64>) {
    out.clear();
    out.resize(index.row_words(), u64::MAX);
    for (w, slot) in out.iter_mut().enumerate() {
        let mut acc = u64::MAX;
        let mut rest = qbig;
        while rest != 0 {
            acc &= index.name_bigram_word(w, rest.trailing_zeros() as usize);
            rest &= rest - 1;
        }
        *slot = acc;
    }
    mask_tail(index, out);
}

fn for_each_row(words: &[u64], mut visit: impl FnMut(usize)) {
    for (w, word) in words.iter().enumerate() {
        let mut bits = *word;
        while bits != 0 {
            visit(w * 64 + bits.trailing_zeros() as usize);
            bits &= bits.wrapping_sub(1);
        }
    }
}

fn quoted_pairs(
    index: &PackageIndex,
    q: &str,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<(u32, Tier)> {
    let mut cands: Vec<Scored> = Vec::new();
    PREFILTER.with(|cell| {
        let mut words = cell.borrow_mut();
        bigram_words(index, bigram_mask(q.as_bytes()), &mut words);
        for_each_row(&words, |pi| {
            if !filter.matches(index.view(pi), installed) {
                return;
            }
            if !index.name(pi).contains(q) {
                return;
            }
            scored_push(&mut cands, index, pi, Tier::Substring, 0, false);
        });
    });
    to_sorted_pairs(index, cands)
}

struct WorstFirst<'a> {
    index: &'a PackageIndex,
    scored: Scored,
}

impl Ord for WorstFirst<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        scored_ordering(self.index, &other.scored, &self.scored)
    }
}

impl PartialOrd for WorstFirst<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for WorstFirst<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for WorstFirst<'_> {}

fn beats_heap_top(
    index: &PackageIndex,
    heap: &BinaryHeap<WorstFirst<'_>>,
    scored: &Scored,
) -> bool {
    heap.peek()
        .is_some_and(|worst| scored_ordering(index, scored, &worst.scored) == Ordering::Less)
}

fn best_k(index: &PackageIndex, cands: Vec<Scored>, k: usize) -> Vec<Scored> {
    let mut heap: BinaryHeap<WorstFirst<'_>> = BinaryHeap::with_capacity(k);
    for scored in cands {
        if heap.len() == k {
            if !beats_heap_top(index, &heap, &scored) {
                continue;
            }
            heap.pop();
        }
        heap.push(WorstFirst { index, scored });
    }
    heap.into_vec().into_iter().map(|w| w.scored).collect()
}

fn to_sorted_pairs(index: &PackageIndex, cands: Vec<Scored>) -> Vec<(u32, Tier)> {
    let mut cands = match cands.len() > RESULT_LIMIT {
        true => best_k(index, cands, RESULT_LIMIT),
        false => cands,
    };
    cands.sort_by(|a, b| scored_ordering(index, a, b));
    cands
        .into_iter()
        .map(|c| (index.row(c.pkg as usize).id, tier_of_key(c.key)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::{RawPkg, assemble, tokenize};
    use crate::search::tiers::{Candidate, candidate_ordering};
    use std::collections::HashMap;

    fn pkg(id: u32, name: &str, is_repo: bool, popularity: u16) -> RawPkg {
        RawPkg {
            id,
            name: name.to_string(),
            tokens: tokenize(name),
            keywords: Vec::new(),
            popularity,
            is_repo,
        }
    }

    fn index_with(raws: Vec<RawPkg>) -> PackageIndex {
        assemble(raws)
    }

    #[test]
    fn gather_cheap_candidates_uses_inverted_ranges() {
        let index = index_with(vec![
            pkg(1, "vim", false, 0),
            pkg(2, "vim-plugins", false, 0),
            pkg(3, "recycle-bin", false, 0),
            pkg(4, "binary", false, 0),
            pkg(5, "visual-vim", false, 0),
        ]);
        let installed: HashSet<String> = HashSet::new();

        let exact_name = gather_cheap_candidates(
            &index,
            "vim",
            &[Tier::ExactName],
            SearchFilter::All,
            &installed,
        );
        assert_eq!(ids_of(&index, &exact_name), vec![1]);

        let exact_token = gather_cheap_candidates(
            &index,
            "bin",
            &[Tier::ExactToken],
            SearchFilter::All,
            &installed,
        );
        assert_eq!(ids_of(&index, &exact_token), vec![3]);

        let prefix_name = gather_cheap_candidates(
            &index,
            "vim",
            &[Tier::PrefixName],
            SearchFilter::All,
            &installed,
        );
        assert_eq!(ids_of(&index, &prefix_name), vec![1, 2]);

        let prefix_token = gather_cheap_candidates(
            &index,
            "vi",
            &[Tier::PrefixToken],
            SearchFilter::All,
            &installed,
        );
        assert_eq!(ids_of(&index, &prefix_token), vec![1, 2, 5]);

        let all_cheap = gather_cheap_candidates(
            &index,
            "vim",
            CHEAP_TIERS_ALL,
            SearchFilter::All,
            &installed,
        );
        assert_eq!(ids_of(&index, &all_cheap), vec![1, 2, 5]);
        assert!(
            all_cheap
                .iter()
                .all(|c| tier_of_key(c.key) != Tier::Substring)
        );
    }

    fn ids_of(index: &PackageIndex, cands: &[Scored]) -> Vec<u32> {
        let mut v: Vec<u32> = cands.iter().map(|c| index.row(c.pkg as usize).id).collect();
        v.sort();
        v
    }

    #[test]
    fn exact_name_ranks_first() {
        let index = index_with(vec![
            pkg(1, "vim", false, 0),
            pkg(2, "vim-plugins", false, 0),
        ]);
        let ids = search_index(&index, "vim");
        assert_eq!(ids.first().copied(), Some(1));
    }

    #[test]
    fn exact_token_outranks_prefix_name() {
        let index = index_with(vec![
            pkg(10, "recycle-bin", false, 0),
            pkg(11, "binary", false, 0),
        ]);
        let ids = search_index(&index, "bin");
        assert_eq!(ids.first().copied(), Some(10));
    }

    #[test]
    fn quoted_strict_substring() {
        let index = index_with(vec![
            pkg(5, "google-chrome", false, 0),
            pkg(6, "chromium", false, 0),
            pkg(7, "firefox", false, 0),
        ]);
        let ids = search_index(&index, "\"chrom\"");
        assert!(ids.contains(&5));
        assert!(!ids.contains(&7));
        assert!(search_index(&index, "\"zzz\"").is_empty());
    }

    #[test]
    fn short_query_restricts_tiers() {
        let index = index_with(vec![
            pkg(1, "ab", false, 0),
            pkg(2, "abc", false, 0),
            pkg(3, "x-ab", false, 0),
            pkg(4, "xxabxx", false, 0),
        ]);
        let ids = search_index(&index, "ab");
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
        assert!(!ids.contains(&4));
    }

    #[test]
    fn normal_fuzzy_fallback_runs_when_concrete_sparse() {
        let index = index_with(vec![pkg(1, "google-chrome", false, 0)]);
        let ids = search_index(&index, "chroem");
        assert!(ids.contains(&1));
    }

    #[test]
    fn normal_fuzzy_skipped_when_concrete_plentiful() {
        let index = index_with(vec![
            pkg(1, "cava", false, 0),
            pkg(2, "cava-foo", false, 0),
            pkg(3, "cava-bar", false, 0),
            pkg(4, "cava-baz", false, 0),
            pkg(5, "cava-qux", false, 0),
            pkg(99, "kava", false, 0),
        ]);
        let ids = search_index(&index, "cava");
        assert!(ids.len() <= RESULT_LIMIT);
        assert!(ids.contains(&1));
        assert!(!ids.contains(&99));
    }

    #[test]
    fn result_limit_truncates_to_thirty() {
        let packages: Vec<RawPkg> = (1u32..=40)
            .map(|i| pkg(i, &format!("prefix-{i}"), false, 0))
            .collect();
        let index = index_with(packages);
        let ids = search_index(&index, "prefix");
        assert_eq!(ids.len(), 30);
    }

    #[test]
    fn overflowing_candidates_keep_the_best_thirty_ranked() {
        let packages: Vec<RawPkg> = (1u32..=40)
            .map(|i| pkg(i, &format!("prefix-{i}"), false, 0))
            .collect();
        let index = index_with(packages);
        let ids = search_index(&index, "prefix");
        assert_eq!(ids, (1u32..=30).collect::<Vec<u32>>());
    }

    #[test]
    fn exact_token_block_is_sorted_so_the_first_thirty_are_the_best_thirty() {
        let packages: Vec<RawPkg> = (1u32..=60)
            .map(|i| RawPkg {
                id: i,
                name: format!("tool-git-{}", "x".repeat((i % 7) as usize)),
                tokens: vec!["git".to_string()],
                keywords: Vec::new(),
                popularity: (i * 37 % 1000) as u16,
                is_repo: i % 5 == 0,
            })
            .collect();
        let index = index_with(packages);

        let mut expected: Vec<Candidate<'_>> = (0..index.len())
            .map(|pi| Candidate {
                view: index.view(pi),
                tier: Tier::ExactToken,
                distance: 0,
                first_letter_match: false,
            })
            .collect();
        expected.sort_by(candidate_ordering);

        let got: Vec<u32> = search_index(&index, "git");
        assert_eq!(got.len(), RESULT_LIMIT);
        assert_eq!(
            got,
            expected[..RESULT_LIMIT]
                .iter()
                .map(|c| c.view.id)
                .collect::<Vec<u32>>()
        );
    }

    #[test]
    fn truncation_excludes_substring_only_when_limit_full() {
        let mut packages: Vec<RawPkg> = (1u32..=RESULT_LIMIT as u32)
            .map(|i| pkg(i, &format!("xxx-{i:03}"), false, 0))
            .collect();
        packages.push(pkg(999, "zzxxxzz", false, 0));
        let index = index_with(packages);

        let ids = search_index(&index, "xxx");

        assert_eq!(ids.len(), RESULT_LIMIT);
        assert!(
            !ids.contains(&999),
            "substring-only package must be dropped once the limit is full"
        );
    }

    fn ids_of_pairs(pairs: &[(u32, Tier)]) -> Vec<u32> {
        let mut v: Vec<u32> = pairs.iter().map(|(id, _)| *id).collect();
        v.sort();
        v
    }

    #[test]
    fn filter_official_returns_only_repo_packages() {
        let index = index_with(vec![pkg(1, "vim", true, 0), pkg(2, "vim", false, 0)]);
        let installed: HashSet<String> = HashSet::new();
        let ids = ids_of_pairs(&search_index_tiered(
            &index,
            "vim",
            SearchFilter::Official,
            &installed,
        ));
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn filter_aur_returns_only_non_repo_packages() {
        let index = index_with(vec![pkg(1, "vim", true, 0), pkg(2, "vim", false, 0)]);
        let installed: HashSet<String> = HashSet::new();
        let ids = ids_of_pairs(&search_index_tiered(
            &index,
            "vim",
            SearchFilter::Aur,
            &installed,
        ));
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn filter_installed_returns_only_installed_names() {
        let index = index_with(vec![pkg(1, "vim", false, 0), pkg(2, "vile", false, 0)]);
        let installed: HashSet<String> = ["vim".to_string()].into_iter().collect();
        let ids = ids_of_pairs(&search_index_tiered(
            &index,
            "vi",
            SearchFilter::Installed,
            &installed,
        ));
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn filter_fill_to_limit_when_all_admitted() {
        let mut raws: Vec<RawPkg> = (1u32..=35)
            .map(|i| pkg(i, &format!("prefix-{i}"), false, 0))
            .collect();
        for (i, c) in (36u32..=40).zip("abcde".chars()) {
            raws.push(pkg(i, &format!("prefix{c}"), false, 0));
        }
        let by_id: HashMap<u32, String> = raws.iter().map(|p| (p.id, p.name.clone())).collect();
        let installed: HashSet<String> = (1u32..=35).map(|i| format!("prefix-{i}")).collect();
        let index = index_with(raws);
        let pairs = search_index_tiered(&index, "prefix", SearchFilter::Installed, &installed);
        assert_eq!(
            pairs.len(),
            RESULT_LIMIT,
            "filter must admit enough installed packages to fill the result limit"
        );
        assert!(
            pairs
                .iter()
                .all(|(id, _)| installed.contains(by_id[id].as_str())),
            "every returned package must be an installed package"
        );
    }

    #[test]
    fn gate_semantics_fuzzy_runs_when_filter_empties_cheap_below_gate() {
        let index = index_with(vec![
            pkg(1, "cava", true, 0),
            pkg(2, "cava-foo", false, 0),
            pkg(3, "cava-bar", false, 0),
            pkg(4, "cava-baz", false, 0),
            pkg(5, "cava-qux", false, 0),
            pkg(6, "cava-quux", false, 0),
            pkg(7, "cavb", true, 0),
        ]);
        let installed: HashSet<String> = HashSet::new();
        let ids = ids_of_pairs(&search_index_tiered(
            &index,
            "cava",
            SearchFilter::Official,
            &installed,
        ));
        assert!(ids.contains(&1), "exact official match must be present");
        assert!(ids.contains(&7), "fuzzy official match must be present");
        assert!(
            !ids.iter().any(|&id| (2..=6).contains(&id)),
            "AUR packages must be excluded"
        );
    }

    #[test]
    fn prefilter_excludes_only_rows_that_cannot_match() {
        let raws: Vec<RawPkg> = (0u32..300)
            .map(|i| {
                let mut p = pkg(i + 1, &format!("pkg-{i}-alpha-beta-gamma"), false, 0);
                p.keywords = if i % 3 == 0 {
                    vec!["alphx".to_string()]
                } else {
                    vec!["zulu".to_string()]
                };
                p
            })
            .chain((300u32..340).map(|i| {
                let mut p = pkg(i + 1, &format!("node-js-{i}"), false, 0);
                p.keywords = vec!["nodejs".to_string()];
                p
            }))
            .collect();
        let index = index_with(raws);

        for q in ["alpha", "node", "alphaq", "alphabet", "ndej", "zz"] {
            let qmask = byte_mask(q.as_bytes());
            if qmask.count_ones() < 3 {
                continue;
            }
            let mut words: Vec<u64> = Vec::new();
            prefilter_words(&index, qmask, &mut words);
            for i in 0..index.len() {
                let r = index.row(i);
                let name_missing = qmask & !r.name_mask;
                let name_arm = at_most_two_missing(name_missing);
                let kw_arm = qmask & r.kw_mask == qmask;
                let set = words[i / 64] >> (i % 64) & 1 == 1;
                if set {
                    continue;
                }
                assert!(
                    !name_arm && !kw_arm,
                    "query {q} row {i} passes the gates but the prefilter dropped it"
                );
            }
        }
    }

    #[test]
    fn quoted_query_respects_filter() {
        let index = index_with(vec![
            pkg(1, "google-chrome", true, 0),
            pkg(2, "google-chrome", false, 0),
        ]);
        let installed: HashSet<String> = HashSet::new();
        let official_ids = ids_of_pairs(&search_index_tiered(
            &index,
            "\"chrome\"",
            SearchFilter::Official,
            &installed,
        ));
        assert_eq!(official_ids, vec![1]);
        let aur_ids = ids_of_pairs(&search_index_tiered(
            &index,
            "\"chrome\"",
            SearchFilter::Aur,
            &installed,
        ));
        assert_eq!(aur_ids, vec![2]);
    }
}

#[cfg(test)]
mod search_engine_tests {
    use super::*;
    use crate::db::PackageDb;
    use crate::search::index::index_path;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::UNIX_EPOCH;

    fn seed_package(conn: &rusqlite::Connection, name: &str, source: &str) {
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params![name, source, source, "1.0-1", "", 0i64, 0.0f64, 0i64, name],
        )
        .expect("seed package");
    }

    fn rowid_to_name(sqlite_path: &Path) -> HashMap<u32, String> {
        let conn = rusqlite::Connection::open(sqlite_path).expect("open read conn");
        let mut stmt = conn
            .prepare("SELECT rowid, name FROM packages")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                Ok((id as u32, name))
            })
            .expect("query");
        rows.filter_map(Result::ok).collect()
    }

    #[test]
    fn search_engine_returns_ranked_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");
        seed_package(&conn, "chromium", "repo");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");

        let ids = engine.search("chrome");
        assert!(!ids.is_empty(), "chrome query must return results");

        let names = rowid_to_name(&sqlite_path);
        let top_name = ids
            .first()
            .and_then(|id| names.get(id))
            .expect("top id maps to a seeded package");
        assert_eq!(top_name, "google-chrome");
    }

    #[test]
    fn ensure_fresh_is_noop_when_index_fresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");
        let before = engine.search("chrome");

        engine.ensure_fresh().expect("ensure_fresh on fresh index");

        let after = engine.search("chrome");
        assert_eq!(before, after, "fresh index must not be rebuilt");
    }

    #[test]
    fn ensure_fresh_rebuilds_when_index_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite_path = dir.path().join("aur-meta.sqlite");
        let _local = PackageDb::open(&sqlite_path).expect("open");
        let conn = rusqlite::Connection::open(&sqlite_path).expect("seed conn");
        seed_package(&conn, "google-chrome", "aur");

        let engine = SearchEngine::new(sqlite_path.clone()).expect("engine");
        assert!(
            engine.search("firefox").is_empty(),
            "firefox absent before rebuild"
        );

        seed_package(&conn, "firefox", "aur");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint");

        let index_bin = index_path(&sqlite_path);
        std::fs::File::open(&index_bin)
            .and_then(|f| f.set_modified(UNIX_EPOCH))
            .expect("backdate index.bin");

        engine.ensure_fresh().expect("ensure_fresh after stale");

        let names = rowid_to_name(&sqlite_path);
        let ids = engine.search("firefox");
        assert!(!ids.is_empty(), "firefox must appear after rebuild");
        let top = ids
            .first()
            .and_then(|id| names.get(id))
            .expect("top id maps to a seeded package");
        assert_eq!(top, "firefox");
    }
}

#[cfg(test)]
mod multi_term_tests {
    use super::*;
    use crate::search::index::{RawPkg, assemble, tokenize};

    fn pkg(id: u32, name: &str, popularity: u16) -> RawPkg {
        RawPkg {
            id,
            name: name.to_string(),
            tokens: tokenize(name),
            keywords: Vec::new(),
            popularity,
            is_repo: false,
        }
    }

    fn cosmic_index() -> PackageIndex {
        assemble(vec![
            pkg(1, "cosmic-files", 900),
            pkg(2, "cosmic-files-git", 10),
            pkg(3, "cosmic-edit", 500),
            pkg(4, "cosmic-edit-git", 5),
            pkg(5, "cosmic-term", 400),
            pkg(6, "cosign", 800),
            pkg(7, "files", 700),
            pkg(8, "file-roller", 600),
        ])
    }

    fn search(index: &PackageIndex, q: &str) -> Vec<(u32, Tier)> {
        search_index_tiered(index, q, SearchFilter::All, &HashSet::new())
    }

    #[test]
    fn space_separated_query_finds_hyphenated_name() {
        let index = cosmic_index();
        let pairs = search(&index, "cosmic files");
        assert_eq!(pairs.first().map(|(id, _)| *id), Some(1));
        assert!(pairs.iter().any(|(id, _)| *id == 2));
        assert!(!pairs.iter().any(|(id, _)| *id == 6));
        assert!(!pairs.iter().any(|(id, _)| *id == 8));
    }

    #[test]
    fn reordered_terms_still_match() {
        let index = cosmic_index();
        let pairs = search(&index, "files cosmic");
        assert_eq!(pairs.first().map(|(id, _)| *id), Some(1));
    }

    #[test]
    fn every_term_must_match() {
        let index = cosmic_index();
        let ids: Vec<u32> = search(&index, "cosmic edit")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids.first(), Some(&3));
        assert!(!ids.contains(&1));
        assert!(!ids.contains(&7));
    }

    #[test]
    fn normalized_match_outranks_token_match() {
        let index = cosmic_index();
        let pairs = search(&index, "cosmic files");
        assert_eq!(pairs[0], (1, Tier::ExactName));
        assert_eq!(pairs[1], (2, Tier::ExactToken));
    }

    #[test]
    fn unmatched_terms_are_dropped_not_fatal() {
        let index = cosmic_index();
        let ids: Vec<u32> = search(&index, "cosmic filez qqq")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(!ids.is_empty());
        assert!(ids.contains(&1));
        assert!(!ids.contains(&7));
        assert!(!ids.contains(&8));
    }
}
