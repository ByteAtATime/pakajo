use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::fuzzy::{FuzzyMatcher, MAX_EDIT_DISTANCE, at_most_two_missing};
use crate::index::{IndexRow, PackageIndex, bigram_mask, build_from_rows, byte_mask, trigram_mask};
use crate::query::{ParsedQuery, parse_query};
use crate::tiers::{ALL_TIERS, Scored, Tier, pack_sort_key, tier_at};
use crate::{PackageGroup, SearchResult, Source};
use crate::{SearchError, SearchFilter};

const RESULT_LIMIT: usize = 30;
const FUZZY_GATE: usize = 5;
const MIN_PREFILTER_CHARS: u32 = 3;

const SHORT_TIERS: &[Tier] = &[Tier::ExactName, Tier::ExactToken, Tier::PrefixName];
const CHEAP_TIERS_ALL: &[Tier] = &[
    Tier::ExactName,
    Tier::ExactToken,
    Tier::PrefixName,
    Tier::PrefixToken,
];
const EXPENSIVE_TIERS: &[Tier] = &[Tier::Substring, Tier::Keyword];

#[cfg(test)]
pub(crate) fn search_index(index: &PackageIndex, text: &str) -> Vec<u32> {
    search_index_tiered(index, text, SearchFilter::All, &HashSet::new())
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

pub(crate) fn search_index_tiered(
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
    cache_path: Option<PathBuf>,
    state: RwLock<EngineState>,
}

struct EngineState {
    index: Arc<PackageIndex>,
    cached_fp: u64,
}

impl SearchEngine {
    pub fn build<F>(cache: Option<(&Path, u64)>, rows: F) -> Result<Self, SearchError>
    where
        F: Fn() -> Result<Vec<IndexRow>, SearchError>,
    {
        let Some((path, fingerprint)) = cache else {
            return Self::without_cache(rows()?);
        };
        match PackageIndex::load(path) {
            Ok((index, stored_fp)) if stored_fp == fingerprint => Ok(Self {
                cache_path: Some(path.to_path_buf()),
                state: RwLock::new(EngineState {
                    index: Arc::new(index),
                    cached_fp: fingerprint,
                }),
            }),
            Ok(_) => Self::miss(path, fingerprint, rows()?),
            Err(SearchError::Corrupt) => Err(SearchError::Corrupt),
            Err(SearchError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Self::miss(path, fingerprint, rows()?)
            }
            Err(other) => Err(other),
        }
    }

    pub fn rebuild<F>(&self, fingerprint: u64, rows: F) -> Result<(), SearchError>
    where
        F: Fn() -> Result<Vec<IndexRow>, SearchError>,
    {
        if self.cached_fp() == fingerprint {
            return Ok(());
        }
        let built = build_from_rows(rows()?);
        let (snapshot, cache_path) = {
            let mut state = self.state.write().expect("index lock poisoned");
            if state.cached_fp == fingerprint {
                return Ok(());
            }
            state.index = Arc::new(built);
            state.cached_fp = fingerprint;
            (state.index.clone(), self.cache_path.clone())
        };
        if fingerprint != u64::MAX
            && let Some(path) = cache_path
        {
            snapshot.store(&path, fingerprint)?;
        }
        Ok(())
    }

    fn without_cache(rows: Vec<IndexRow>) -> Result<Self, SearchError> {
        Ok(Self {
            cache_path: None,
            state: RwLock::new(EngineState {
                index: Arc::new(build_from_rows(rows)),
                cached_fp: u64::MAX,
            }),
        })
    }

    fn miss(path: &Path, fingerprint: u64, rows: Vec<IndexRow>) -> Result<Self, SearchError> {
        let index = build_from_rows(rows);
        if fingerprint != u64::MAX {
            index.store(path, fingerprint)?;
        }
        Ok(Self {
            cache_path: Some(path.to_path_buf()),
            state: RwLock::new(EngineState {
                index: Arc::new(index),
                cached_fp: fingerprint,
            }),
        })
    }

    fn cached_fp(&self) -> u64 {
        self.state.read().expect("index lock poisoned").cached_fp
    }

    pub(crate) fn search_tiered(
        &self,
        text: &str,
        filter: SearchFilter,
        installed: &HashSet<String>,
    ) -> Vec<(u32, Tier)> {
        let snapshot = self
            .state
            .read()
            .expect("index lock poisoned")
            .index
            .clone();
        search_index_tiered(&snapshot, text, filter, installed)
    }

    pub fn complete_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let snapshot = self
            .state
            .read()
            .expect("index lock poisoned")
            .index
            .clone();
        if prefix.is_empty() {
            return snapshot
                .names_sorted
                .iter()
                .take(limit)
                .map(|&pi| snapshot.name(pi as usize).to_owned())
                .collect();
        }
        let folded = prefix.to_lowercase();
        snapshot
            .prefix_name_range(folded.as_bytes())
            .take(limit)
            .map(|i| snapshot.name(snapshot.names_sorted[i] as usize).to_owned())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.state.read().expect("index lock poisoned").index.len() == 0
    }

    pub fn query(&self, text: &str) -> SearchQuery<'_> {
        SearchQuery {
            engine: self,
            text: text.to_owned(),
            filter: SearchFilter::All,
            installed: HashSet::new(),
            groups: Vec::new(),
            limit: RESULT_LIMIT,
        }
    }
}

pub struct SearchQuery<'a> {
    engine: &'a SearchEngine,
    text: String,
    filter: SearchFilter,
    installed: HashSet<String>,
    groups: Vec<PackageGroup>,
    limit: usize,
}

impl<'a> SearchQuery<'a> {
    pub fn filter(mut self, filter: SearchFilter) -> Self {
        self.filter = filter;
        self
    }

    pub fn installed(mut self, installed: &HashSet<String>) -> Self {
        self.installed = installed.iter().map(|name| name.to_lowercase()).collect();
        self
    }

    pub fn groups(mut self, groups: &[(String, String)]) -> Self {
        self.groups = groups
            .iter()
            .map(|(name, repo)| PackageGroup {
                name: name.clone(),
                repo: repo.clone(),
            })
            .collect();
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn execute<H>(self, hydrate: H) -> Result<Vec<SearchResult>, SearchError>
    where
        H: Fn(&[u32]) -> Result<HashMap<u32, SearchResult>, SearchError>,
    {
        let mut pairs = self
            .engine
            .search_tiered(&self.text, self.filter, &self.installed);
        pairs.truncate(self.limit);
        let ids: Vec<u32> = pairs.iter().map(|(id, _)| *id).collect();
        let mut results = hydrate(&ids)?;
        let mut entries: Vec<(Tier, SearchResult)> = Vec::with_capacity(pairs.len());
        for (id, tier) in &pairs {
            if let Some(mut result) = results.remove(id) {
                result.installed = self.installed.contains(&result.name.to_lowercase());
                entries.push((*tier, result));
            }
        }
        let query = self.text.to_lowercase();
        if !query.is_empty() && self.filter == SearchFilter::All {
            for group in &self.groups {
                if let Some(tier) = group_name_tier(&group.name.to_lowercase(), &query) {
                    entries.push((
                        tier,
                        SearchResult {
                            name: group.name.clone(),
                            source: Source::Group,
                            description: Some("group".to_string()),
                            version: Some(String::new()),
                            repo: Some(group.repo.clone()),
                            installed: false,
                            num_votes: 0,
                            popularity: 0.0,
                            last_update: 0,
                        },
                    ));
                }
            }
        }
        entries.sort_by_key(|entry| entry.0);
        Ok(entries.into_iter().map(|(_, result)| result).collect())
    }
}

fn group_name_tier(name: &str, query: &str) -> Option<Tier> {
    if name == query {
        Some(Tier::ExactName)
    } else if name.starts_with(query) {
        Some(Tier::PrefixName)
    } else if name.contains(query) {
        Some(Tier::Substring)
    } else {
        None
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
        tier,
        key: pack_sort_key(distance, first_letter_match, index.rank_word(pkg)),
        pkg: pkg as u32,
    });
}

fn scored_ordering(index: &PackageIndex, a: &Scored, b: &Scored) -> Ordering {
    a.tier
        .cmp(&b.tier)
        .then_with(|| a.key.cmp(&b.key))
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
                if filter.row_matches(index, pkg, installed) {
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
        if filter.row_matches(index, i, installed)
            && let Some(tier) = tier_at(index, i, q, EXPENSIVE_TIERS, qmask, qbig)
        {
            scored_push(&mut cands, index, i, tier, 0, false);
        }
    }
    cands
}

struct TermMasks {
    chars: u64,
    bigrams: u64,
    trigrams: u64,
}

impl TermMasks {
    fn of(term: &str) -> Self {
        Self {
            chars: byte_mask(term.as_bytes()),
            bigrams: bigram_mask(term.as_bytes()),
            trigrams: trigram_mask(term.as_bytes()),
        }
    }
}

fn word_map(index: &PackageIndex, out: &mut Vec<u64>, word: impl Fn(usize) -> u64) {
    out.clear();
    out.resize(index.row_words(), 0);
    for (w, slot) in out.iter_mut().enumerate() {
        *slot = word(w);
    }
    mask_tail(index, out);
}

fn seed_words(index: &PackageIndex, masks: &TermMasks, out: &mut Vec<u64>) {
    word_map(index, out, |w| {
        let name = all_present(masks.chars, |b| index.name_char_word(w, b))
            & all_present(masks.bigrams, |b| index.name_bigram_word(w, b))
            & all_present(masks.trigrams, |b| index.name_trigram_word(w, b));
        let kw = all_present(masks.chars, |b| index.kw_char_word(w, b))
            & all_present(masks.bigrams, |b| index.kw_bigram_word(w, b))
            & all_present(masks.trigrams, |b| index.kw_trigram_word(w, b));
        name | kw
    });
}

fn for_each_seed_row(
    index: &PackageIndex,
    term: &str,
    masks: &TermMasks,
    filter: SearchFilter,
    installed: &HashSet<String>,
    mut visit: impl FnMut(usize, Tier) -> ControlFlow<()>,
) {
    PREFILTER.with(|cell| {
        let mut words = cell.borrow_mut();
        seed_words(index, masks, &mut words);
        for_each_row(&words, |pkg| {
            term_tier_at(index, pkg, term, masks, filter, installed)
                .map_or(ControlFlow::Continue(()), |tier| visit(pkg, tier))
        });
    });
}

fn seed_rows(
    index: &PackageIndex,
    term: &str,
    masks: &TermMasks,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Vec<(usize, Tier)> {
    let mut out: Vec<(usize, Tier)> = Vec::new();
    for_each_seed_row(index, term, masks, filter, installed, |pkg, tier| {
        out.push((pkg, tier));
        ControlFlow::Continue(())
    });
    out
}

fn term_tier_at(
    index: &PackageIndex,
    pkg: usize,
    term: &str,
    masks: &TermMasks,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> Option<Tier> {
    if !filter.row_matches(index, pkg, installed) {
        return None;
    }
    tier_at(index, pkg, term, ALL_TIERS, masks.chars, masks.bigrams)
}

fn term_matches_any(
    index: &PackageIndex,
    term: &str,
    masks: &TermMasks,
    filter: SearchFilter,
    installed: &HashSet<String>,
) -> bool {
    let mut matched = false;
    for_each_seed_row(index, term, masks, filter, installed, |_, _| {
        matched = true;
        ControlFlow::Break(())
    });
    matched
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
        if (nqmask & !r.name_mask) != 0 || !filter.row_matches(index, pkg, installed) {
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
        let masks = TermMasks::of(term);
        if survivors.is_empty() {
            survivors = seed_rows(index, term, &masks, filter, installed);
            continue;
        }
        let mut kept: Vec<(usize, Tier)> = Vec::new();
        for (pkg, tier) in survivors.iter().copied() {
            if let Some(other) = term_tier_at(index, pkg, term, &masks, filter, installed) {
                kept.push((pkg, tier.max(other)));
            }
        }
        if kept.is_empty() {
            if term_matches_any(index, term, &masks, filter, installed) {
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
struct QueryChars {
    mask: u64,
    non_ascii: usize,
    count: usize,
}

impl QueryChars {
    fn new(q: &str) -> Self {
        Self {
            mask: byte_mask(q.as_bytes()),
            non_ascii: q.chars().filter(|c| !c.is_ascii()).count(),
            count: q.chars().count(),
        }
    }
}

fn char_gate_passes(
    qc: &QueryChars,
    cand_mask: u64,
    cand_is_ascii: bool,
    cand_chars: usize,
) -> bool {
    let missing = qc.mask & !cand_mask;
    let extra = cand_mask & !qc.mask;
    if !cand_is_ascii {
        return missing.count_ones() as usize + extra.count_ones() as usize <= MAX_EDIT_DISTANCE;
    }
    if cand_chars.abs_diff(qc.count) > MAX_EDIT_DISTANCE {
        return false;
    }
    if qc.non_ascii == 0 {
        return at_most_two_missing(missing) && at_most_two_missing(extra);
    }
    missing.count_ones() as usize + qc.non_ascii <= MAX_EDIT_DISTANCE && at_most_two_missing(extra)
}

#[derive(Clone, Copy)]
struct FusedCtx<'a> {
    index: &'a PackageIndex,
    q: &'a str,
    chars: QueryChars,
    qbig: u64,
    q_first: Option<char>,
    filter: SearchFilter,
    installed: &'a HashSet<String>,
}

fn prefilter_words(index: &PackageIndex, qmask: u64, out: &mut Vec<u64>) {
    word_map(index, out, |w| {
        at_most_two_absent(qmask, |b| !index.name_char_word(w, b))
            | all_present(qmask, |b| index.kw_char_word(w, b))
    });
}

fn at_most_two_absent(qmask: u64, absent: impl Fn(usize) -> u64) -> u64 {
    let (mut once, mut twice, mut thrice) = (0u64, 0u64, 0u64);
    let mut rest = qmask;
    while rest != 0 {
        let bit = absent(rest.trailing_zeros() as usize);
        thrice |= twice & bit;
        twice |= once & bit;
        once |= bit;
        rest &= rest - 1;
    }
    !thrice
}

fn all_present(qmask: u64, present: impl Fn(usize) -> u64) -> u64 {
    let mut rows = u64::MAX;
    let mut rest = qmask;
    while rest != 0 {
        rows &= present(rest.trailing_zeros() as usize);
        rest &= rest - 1;
    }
    rows
}

fn full_words(index: &PackageIndex, out: &mut Vec<u64>) {
    word_map(index, out, |_| u64::MAX);
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
        chars,
        qbig,
        q_first,
        filter,
        installed,
    } = *ctx;
    let r = index.row(i);
    let name_tier_arm = r.name_len as usize >= q.len() && (qbig & !r.name_bigrams) == 0;
    let kw_tier_arm = index.max_kw_len(i) as usize >= q.len() && (qbig & !r.kw_bigrams) == 0;
    if (name_tier_arm || kw_tier_arm)
        && let Some(tier) = tier_at(index, i, q, EXPENSIVE_TIERS, chars.mask, qbig)
    {
        if filter.row_matches(index, i, installed) {
            scored_push(cands, index, i, tier, 0, false);
        }
        placed.insert(i);
        return;
    }
    if !char_gate_passes(&chars, r.name_mask, r.ascii_name, r.name_len as usize) {
        return;
    }
    let Some(name_d) =
        matcher.within_distance(index.name(i).as_bytes(), r.name_mask, MAX_EDIT_DISTANCE)
    else {
        return;
    };
    if !filter.row_matches(index, i, installed) {
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

fn seed_fuzzy_token(
    ctx: &FusedCtx,
    tid: usize,
    token_mask: u64,
    ascii_token: bool,
    matcher: &mut FuzzyMatcher,
    fuzzy_buf: &mut Vec<Scored>,
    placed: &mut EpochSet,
) {
    let FusedCtx {
        index,
        chars,
        q_first,
        filter,
        installed,
        ..
    } = *ctx;
    if !char_gate_passes(&chars, token_mask, ascii_token, index.token_len(tid)) {
        return;
    }
    let token = index.token_str(tid);
    let Some(seed_distance) =
        matcher.within_distance(token.as_bytes(), token_mask, MAX_EDIT_DISTANCE)
    else {
        return;
    };
    for j in index.exact_token_range(token.as_bytes()) {
        let pkg = index.tokens_sorted[j].1 as usize;
        if placed.contains(pkg) {
            continue;
        }
        if !filter.row_matches(index, pkg, installed) {
            placed.insert(pkg);
            continue;
        }
        let seed_first_letter = token.chars().next() == q_first;
        let (distance, first_letter_match) = fuzzy_score_from_seed(
            matcher,
            index,
            pkg,
            seed_distance as u8,
            seed_first_letter,
            q_first,
        );
        scored_push(
            fuzzy_buf,
            index,
            pkg,
            Tier::Fuzzy,
            distance,
            first_letter_match,
        );
        placed.insert(pkg);
    }
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
    let chars = QueryChars::new(q);
    let ctx = FusedCtx {
        index,
        q,
        chars,
        qbig: bigram_mask(q.as_bytes()),
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
        if ctx.chars.mask.count_ones() >= MIN_PREFILTER_CHARS {
            prefilter_words(index, ctx.chars.mask, &mut words);
        } else {
            full_words(index, &mut words);
        }
        for_each_row(&words, |i| {
            if !seen.contains(i) {
                fused_expensive_row(&ctx, i, &mut matcher, &mut cands, &mut fuzzy_buf, placed);
            }
            ControlFlow::Continue(())
        });
    });

    let shortest = ctx.chars.count.saturating_sub(MAX_EDIT_DISTANCE);
    let longest = ctx.chars.count + MAX_EDIT_DISTANCE;
    for len in shortest..=longest {
        for slot in index.token_bucket(len) {
            seed_fuzzy_token(
                &ctx,
                index.token_scan_ids[slot] as usize,
                index.token_scan_masks[slot],
                true,
                &mut matcher,
                &mut fuzzy_buf,
                placed,
            );
        }
    }
    for &tid in &index.token_scan_nonascii {
        seed_fuzzy_token(
            &ctx,
            tid as usize,
            index.unique_token_masks[tid as usize],
            false,
            &mut matcher,
            &mut fuzzy_buf,
            placed,
        );
    }

    if cands.len() < FUZZY_GATE {
        cands.append(&mut fuzzy_buf);
    }
    cands
}

fn bigram_words(index: &PackageIndex, qbig: u64, out: &mut Vec<u64>) {
    word_map(index, out, |w| {
        all_present(qbig, |b| index.name_bigram_word(w, b))
    });
}

fn for_each_row(words: &[u64], mut visit: impl FnMut(usize) -> ControlFlow<()>) {
    for (w, word) in words.iter().enumerate() {
        let mut bits = *word;
        while bits != 0 {
            if visit(w * 64 + bits.trailing_zeros() as usize).is_break() {
                return;
            }
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
            if filter.row_matches(index, pi, installed) && index.name(pi).contains(q) {
                scored_push(&mut cands, index, pi, Tier::Substring, 0, false);
            }
            ControlFlow::Continue(())
        });
    });
    to_sorted_pairs(index, cands)
}

fn best_k(index: &PackageIndex, mut cands: Vec<Scored>, k: usize) -> Vec<Scored> {
    if cands.len() > k {
        cands.select_nth_unstable_by(k, |a, b| scored_ordering(index, a, b));
        cands.truncate(k);
    }
    cands
}

fn to_sorted_pairs(index: &PackageIndex, cands: Vec<Scored>) -> Vec<(u32, Tier)> {
    let mut cands = match cands.len() > RESULT_LIMIT {
        true => best_k(index, cands, RESULT_LIMIT),
        false => cands,
    };
    cands.sort_by(|a, b| scored_ordering(index, a, b));
    cands
        .into_iter()
        .map(|c| (index.row(c.pkg as usize).id, c.tier))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{RawPkg, assemble, tokenize};
    use crate::tiers::{Candidate, PkgView, candidate_ordering};
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
        assert!(all_cheap.iter().all(|c| c.tier != Tier::Substring));
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
    fn non_ascii_name_matches_fuzzy_query() {
        let index = index_with(vec![pkg(1, "über", false, 0), pkg(2, "zunder", false, 0)]);
        let ids = search_index(&index, "yber");
        assert!(ids.contains(&1));
    }

    #[test]
    fn non_ascii_token_matches_beyond_the_length_window() {
        let index = index_with(vec![RawPkg {
            id: 7,
            name: "zzz-tool".to_string(),
            tokens: vec!["üüber".to_string()],
            keywords: Vec::new(),
            popularity: 0,
            is_repo: false,
        }]);
        let ids = search_index(&index, "über");
        assert_eq!(ids.first().copied(), Some(7));
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

    fn view_of<'a>(index: &'a PackageIndex, pi: usize) -> PkgView<'a> {
        let r = index.row(pi);
        PkgView {
            name: index.name(pi),
            id: r.id,
            popularity: r.popularity,
            is_repo: r.is_repo,
        }
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
                view: view_of(&index, pi),
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
    fn prefilter_keeps_a_row_missing_two_of_three_query_chars() {
        let index = index_with(vec![pkg(1, "abcde", false, 0)]);
        let qmask = byte_mask(b"axy");
        assert!(at_most_two_missing(qmask & !index.row(0).name_mask));
        let mut words: Vec<u64> = Vec::new();
        prefilter_words(&index, qmask, &mut words);
        assert_eq!(
            words[0] & 1,
            1,
            "a row within edit distance 2 must survive the prefilter"
        );
    }

    #[test]
    fn result_limit_keeps_the_best_thirty_not_the_first_thirty() {
        let packages: Vec<RawPkg> = (0..40u32)
            .map(|i| {
                let name = format!("t-{i:02}");
                RawPkg {
                    id: i + 1,
                    name: name.clone(),
                    tokens: tokenize(&name),
                    keywords: Vec::new(),
                    popularity: i as u16,
                    is_repo: false,
                }
            })
            .collect();
        let index = index_with(packages);
        assert_eq!(
            search_index(&index, "t-"),
            (11..=40u32).rev().collect::<Vec<u32>>()
        );
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
mod multi_term_tests {
    use super::*;
    use crate::index::{RawPkg, assemble, tokenize};

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
    fn keyword_tier_matches_survive_the_seed_gate() {
        let index = assemble(vec![RawPkg {
            id: 1,
            name: "zzz-tool".to_string(),
            tokens: tokenize("zzz-tool"),
            keywords: vec!["node runtime".to_string()],
            popularity: 0,
            is_repo: false,
        }]);
        let pairs = search(&index, "node runtime");
        assert_eq!(pairs.first().map(|(id, _)| *id), Some(1));
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

#[cfg(test)]
mod query_api_tests {
    use super::*;
    use crate::Source;
    use crate::index::IndexRow;
    use std::collections::HashMap;

    fn index_row(id: u32, name: &str) -> IndexRow {
        IndexRow {
            id,
            name: name.to_string(),
            source: "aur".to_string(),
            popularity: None,
            keywords: None,
        }
    }

    fn test_result(name: &str) -> SearchResult {
        SearchResult {
            name: name.to_string(),
            description: None,
            source: Source::Aur,
            repo: None,
            version: None,
            installed: false,
            last_update: 0,
            num_votes: 0,
            popularity: 0.0,
        }
    }

    fn engine_with(names: &[&str]) -> (SearchEngine, HashMap<u32, SearchResult>) {
        let rows = make_rows(names);
        let metas: HashMap<u32, SearchResult> = rows
            .iter()
            .map(|row| (row.id, test_result(&row.name)))
            .collect();
        let engine = SearchEngine::build(None, || Ok(make_rows(names))).expect("build engine");
        (engine, metas)
    }

    fn make_rows(names: &[&str]) -> Vec<IndexRow> {
        names
            .iter()
            .enumerate()
            .map(|(pos, name)| index_row(pos as u32 + 1, name))
            .collect()
    }

    fn full_hydrator(
        metas: &HashMap<u32, SearchResult>,
    ) -> impl Fn(&[u32]) -> Result<HashMap<u32, SearchResult>, SearchError> + '_ {
        |ids| {
            let mut out = HashMap::with_capacity(ids.len());
            for id in ids {
                if let Some(meta) = metas.get(id) {
                    out.insert(*id, meta.clone());
                }
            }
            Ok(out)
        }
    }

    fn is_group(result: &SearchResult) -> bool {
        result.source == Source::Group
    }

    #[test]
    fn groups_surface_only_for_all_filter_with_nonempty_query() {
        let (engine, metas) = engine_with(&["gnome-shell", "gnome-terminal"]);
        let groups = vec![("gnome".to_string(), "extra".to_string())];

        let all: Vec<SearchResult> = engine
            .query("gnome")
            .groups(&groups)
            .execute(full_hydrator(&metas))
            .expect("execute");
        assert!(
            all.iter()
                .any(|result| is_group(result) && result.name == "gnome"),
            "matching group must surface under the All filter"
        );

        let installed: HashSet<String> = ["gnome-shell".to_string()].into_iter().collect();
        let filtered: Vec<SearchResult> = engine
            .query("gnome")
            .filter(SearchFilter::Installed)
            .installed(&installed)
            .groups(&groups)
            .execute(full_hydrator(&metas))
            .expect("execute");
        assert!(
            filtered.iter().all(|result| !is_group(result)),
            "groups must stay hidden under the Installed filter"
        );

        let empty: Vec<SearchResult> = engine
            .query("")
            .groups(&groups)
            .execute(full_hydrator(&metas))
            .expect("execute");
        assert!(
            empty.iter().all(|result| !is_group(result)),
            "groups must stay hidden for an empty query"
        );
    }

    #[test]
    fn installed_folding_ignores_name_case() {
        let (engine, metas) = engine_with(&["vim"]);
        let upper: HashSet<String> = ["VIM".to_string()].into_iter().collect();
        let results: Vec<SearchResult> = engine
            .query("vim")
            .installed(&upper)
            .execute(full_hydrator(&metas))
            .expect("execute");
        let vim = results
            .iter()
            .find(|result| result.name == "vim")
            .expect("vim hit");
        assert!(vim.installed, "VIM entry must mark vim installed");

        let (engine, metas) = engine_with(&["VIM"]);
        let lower: HashSet<String> = ["vim".to_string()].into_iter().collect();
        let results: Vec<SearchResult> = engine
            .query("vim")
            .installed(&lower)
            .execute(full_hydrator(&metas))
            .expect("execute");
        let vim = results
            .iter()
            .find(|result| result.name == "VIM")
            .expect("VIM hit");
        assert!(vim.installed, "vim entry must mark VIM installed");
    }

    #[test]
    fn default_limit_returns_thirty_packages_plus_two_groups() {
        let names: Vec<String> = (0..35).map(|pos| format!("testpkg-{pos:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (engine, metas) = engine_with(&refs);
        let groups = vec![
            ("testpkg-extra-a".to_string(), "repo".to_string()),
            ("testpkg-extra-b".to_string(), "repo".to_string()),
        ];

        let results: Vec<SearchResult> = engine
            .query("testpkg")
            .groups(&groups)
            .execute(full_hydrator(&metas))
            .expect("execute");

        assert_eq!(results.len(), 32);
        assert_eq!(results.iter().filter(|result| is_group(result)).count(), 2);
        assert_eq!(
            results.iter().filter(|result| !is_group(result)).count(),
            30
        );
    }

    #[test]
    fn hydrator_miss_skips_rows_without_panic() {
        let (engine, metas) = engine_with(&["alpha", "alphabet", "alphasonic"]);
        let partial: HashMap<u32, SearchResult> =
            metas.into_iter().filter(|(id, _)| *id != 2).collect();

        let results: Vec<SearchResult> = engine
            .query("alpha")
            .execute(full_hydrator(&partial))
            .expect("execute");

        let names: Vec<&str> = results.iter().map(|result| result.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"alphasonic"));
        assert!(!names.contains(&"alphabet"));
    }
}

#[cfg(test)]
mod complete_prefix_tests {
    use super::*;
    use crate::index::{IndexRow, RawPkg, assemble, tokenize};

    fn completion_row(id: u32, name: &str) -> IndexRow {
        IndexRow {
            id,
            name: name.to_string(),
            source: "aur".to_string(),
            popularity: None,
            keywords: None,
        }
    }

    fn completion_engine(names: &[&str]) -> SearchEngine {
        SearchEngine::build(None, || {
            Ok(names
                .iter()
                .enumerate()
                .map(|(pos, name)| completion_row(pos as u32 + 1, name))
                .collect())
        })
        .expect("build engine")
    }

    fn corpus() -> SearchEngine {
        completion_engine(&[
            "alpha",
            "alpine",
            "beta",
            "firefox",
            "firefox-bin",
            "firefox-esr",
            "gamma",
        ])
    }

    #[test]
    fn full_name_prefix_returns_name_and_extensions() {
        assert_eq!(
            corpus().complete_prefix("firefox", 10),
            vec![
                "firefox".to_string(),
                "firefox-bin".to_string(),
                "firefox-esr".to_string(),
            ]
        );
    }

    #[test]
    fn between_neighbors_prefix_returns_empty() {
        assert!(corpus().complete_prefix("az", 10).is_empty());
    }

    #[test]
    fn beyond_last_name_prefix_returns_empty() {
        assert!(corpus().complete_prefix("zzz", 10).is_empty());
    }

    #[test]
    fn empty_prefix_returns_first_names_in_order() {
        assert_eq!(
            corpus().complete_prefix("", 3),
            vec![
                "alpha".to_string(),
                "alpine".to_string(),
                "beta".to_string(),
            ]
        );
    }

    #[test]
    fn limit_truncates_match_set_in_order() {
        let names: Vec<String> = (0..10).map(|pos| format!("testpkg-{pos:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            completion_engine(&refs).complete_prefix("testpkg-", 3),
            vec![
                "testpkg-00".to_string(),
                "testpkg-01".to_string(),
                "testpkg-02".to_string(),
            ]
        );
    }

    #[test]
    fn zero_limit_returns_empty() {
        assert!(corpus().complete_prefix("firefox", 0).is_empty());
    }

    #[test]
    fn uppercase_prefix_folds_to_lowercase_rows() {
        assert_eq!(
            corpus().complete_prefix("FIRE", 10),
            vec![
                "firefox".to_string(),
                "firefox-bin".to_string(),
                "firefox-esr".to_string(),
            ]
        );
    }

    #[test]
    fn folded_prefix_skips_mixed_case_row_sorted_before_lowercase() {
        let index = assemble(vec![
            RawPkg {
                id: 1,
                name: "FirefoX".to_string(),
                tokens: tokenize("FirefoX"),
                keywords: Vec::new(),
                popularity: 0,
                is_repo: false,
            },
            RawPkg {
                id: 2,
                name: "firefox".to_string(),
                tokens: tokenize("firefox"),
                keywords: Vec::new(),
                popularity: 0,
                is_repo: false,
            },
            RawPkg {
                id: 3,
                name: "firefox-bin".to_string(),
                tokens: tokenize("firefox-bin"),
                keywords: Vec::new(),
                popularity: 0,
                is_repo: false,
            },
        ]);
        let ordered: Vec<&str> = index
            .names_sorted
            .iter()
            .map(|&pi| index.name(pi as usize))
            .collect();
        assert_eq!(ordered, vec!["FirefoX", "firefox", "firefox-bin"]);
        let matched: Vec<&str> = index
            .prefix_name_range(b"fire")
            .map(|i| index.name(index.names_sorted[i] as usize))
            .collect();
        assert_eq!(matched, vec!["firefox", "firefox-bin"]);
    }

    #[test]
    fn is_empty_tracks_index_contents() {
        assert!(completion_engine(&[]).is_empty());
        assert!(!corpus().is_empty());
    }
}
