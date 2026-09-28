use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::SearchError;
use crate::tiers::rank_bits;

const POP_NORM_MAX: f64 = 100.0;
const INDEX_MAGIC: [u8; 4] = *b"v003";

fn next_prefix_bound(q: &[u8]) -> Option<Vec<u8>> {
    let last = q.len() - 1;
    if q[last] == 0xFF {
        return None;
    }
    let mut upper = q.to_vec();
    upper[last] += 1;
    Some(upper)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PkgRow {
    pub(crate) id: u32,
    name_off: u32,
    pub(crate) name_len: u16,
    norm_off: u32,
    norm_len: u16,
    tokens_start: u32,
    tokens_len: u16,
    kws_start: u32,
    kws_len: u16,
    pub(crate) popularity: u16,
    pub(crate) is_repo: bool,
    pub(crate) ascii_name: bool,
    pub(crate) name_mask: u64,
    pub(crate) kw_mask: u64,
    pub(crate) name_bigrams: u64,
    pub(crate) kw_bigrams: u64,
}

pub(crate) struct RawPkg {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) tokens: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) popularity: u16,
    pub(crate) is_repo: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PackageIndex {
    pub(crate) rows: Vec<PkgRow>,
    pub(crate) rank_bits: Box<[u64]>,
    pub(crate) max_kw_lens: Box<[u16]>,
    pub(crate) arena: Box<str>,
    pub(crate) token_ids: Box<[u32]>,
    pub(crate) kw_ids: Box<[u32]>,
    pub(crate) unique_tokens: Box<[(u32, u16)]>,
    pub(crate) unique_token_masks: Box<[u64]>,
    pub(crate) token_scan_ids: Box<[u32]>,
    pub(crate) token_scan_masks: Box<[u64]>,
    pub(crate) token_scan_starts: Box<[u32]>,
    pub(crate) token_scan_nonascii: Box<[u32]>,
    pub(crate) name_char_words: Box<[u64]>,
    pub(crate) kw_char_words: Box<[u64]>,
    pub(crate) name_bigram_words: Box<[u64]>,
    pub(crate) kw_bigram_words: Box<[u64]>,
    pub(crate) name_trigram_words: Box<[u64]>,
    pub(crate) kw_trigram_words: Box<[u64]>,
    pub(crate) unique_kws: Box<[(u32, u16)]>,
    pub(crate) names_sorted: Vec<u32>,
    pub(crate) tokens_sorted: Vec<(u32, u32)>,
    pub(crate) version: u32,
    pub(crate) built_at: u64,
}

pub fn byte_mask(bytes: &[u8]) -> u64 {
    let mut m: u64 = 0;
    for &b in bytes {
        m |= char_bit(b);
    }
    m
}

pub fn bigram_mask(bytes: &[u8]) -> u64 {
    let mut m: u64 = 0;
    for pair in bytes.windows(2) {
        let bit = (u64::from(pair[0]) * 33 + u64::from(pair[1])) & 63;
        m |= 1 << bit;
    }
    m
}

pub fn trigram_mask(bytes: &[u8]) -> u64 {
    let mut m: u64 = 0;
    for window in bytes.windows(3) {
        let bit =
            ((u64::from(window[0]) * 33 + u64::from(window[1])) * 33 + u64::from(window[2])) & 63;
        m |= 1 << bit;
    }
    m
}

fn char_bit(b: u8) -> u64 {
    match b {
        b'a'..=b'z' => 1u64 << (b - b'a'),
        b'0'..=b'9' => 1u64 << (b - b'0' + 26),
        b'-' => 1u64 << 36,
        b'_' => 1u64 << 37,
        b'.' => 1u64 << 38,
        b'+' => 1u64 << 39,
        _ => 0,
    }
}

pub fn tokenize(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn normalize_popularity(pop: Option<f64>, is_repo: bool) -> u16 {
    if is_repo {
        return 0;
    }
    let Some(pop) = pop else {
        return 0;
    };
    let scaled = (pop / POP_NORM_MAX).clamp(0.0, 1.0) * 65535.0;
    scaled.round().min(65535.0) as u16
}

fn parse_keywords(kw: Option<String>) -> Vec<String> {
    match kw {
        None => Vec::new(),
        Some(s) => s.split_whitespace().map(|w| w.to_lowercase()).collect(),
    }
}

pub struct IndexRow {
    pub id: u32,
    pub name: String,
    pub source: String,
    pub popularity: Option<f64>,
    pub keywords: Option<String>,
}

pub fn build_from_rows(rows: Vec<IndexRow>) -> PackageIndex {
    let raws: Vec<RawPkg> = rows
        .into_iter()
        .map(|row| {
            let is_repo = row.source == "repo";
            let name = row.name.to_lowercase();
            let tokens = tokenize(&row.name);
            let keywords = parse_keywords(row.keywords);
            let popularity = normalize_popularity(row.popularity, is_repo);
            RawPkg {
                id: row.id,
                name,
                tokens,
                keywords,
                popularity,
                is_repo,
            }
        })
        .collect();
    assemble(raws)
}

fn push_span(arena: &mut String, s: &str) -> (u32, u16) {
    let off: u32 = arena.len().try_into().expect("arena offset exceeds u32");
    arena.push_str(s);
    let len: u16 = s.len().try_into().expect("span length exceeds u16");
    (off, len)
}

fn kw_ngram_mask(keywords: &[String], mask: fn(&[u8]) -> u64) -> u64 {
    keywords
        .iter()
        .map(|k| mask(k.as_bytes()))
        .fold(0u64, |acc, m| acc | m)
}

fn build_rows(raws: &[RawPkg], arena: &mut String) -> Vec<PkgRow> {
    let n = raws.len();
    let mut rows: Vec<PkgRow> = Vec::with_capacity(n);
    for raw in raws {
        let (name_off, name_len) = push_span(arena, &raw.name);
        let normalized: String = raw.name.chars().filter(|c| c.is_alphanumeric()).collect();
        let (norm_off, norm_len) = push_span(arena, &normalized);
        let kw_mask = kw_ngram_mask(&raw.keywords, byte_mask);
        let name_mask = byte_mask(raw.name.as_bytes());
        let name_bigrams = bigram_mask(raw.name.as_bytes());
        let kw_bigrams = kw_ngram_mask(&raw.keywords, bigram_mask);
        let tokens_len: u16 = raw
            .tokens
            .len()
            .try_into()
            .expect("token count exceeds u16");
        let kws_len: u16 = raw
            .keywords
            .len()
            .try_into()
            .expect("keyword count exceeds u16");
        rows.push(PkgRow {
            id: raw.id,
            name_off,
            name_len,
            norm_off,
            norm_len,
            tokens_start: 0,
            tokens_len,
            kws_start: 0,
            kws_len,
            popularity: raw.popularity,
            is_repo: raw.is_repo,
            ascii_name: raw.name.is_ascii(),
            name_mask,
            kw_mask,
            name_bigrams,
            kw_bigrams,
        });
    }
    rows
}

type TextSpan = (u32, u16);
type TokenPosting = (u32, u32);

struct TokenInversion {
    token_ids: Vec<u32>,
    unique_tokens: Vec<TextSpan>,
    unique_token_masks: Vec<u64>,
    tokens_sorted: Vec<TokenPosting>,
}

fn build_token_inversion(
    raws: &[RawPkg],
    arena: &mut String,
    rows: &mut [PkgRow],
) -> TokenInversion {
    let n = raws.len();
    let mut occ: Vec<(u32, u32)> = Vec::new();
    for (pi, raw) in raws.iter().enumerate() {
        for ti in 0..raw.tokens.len() {
            occ.push((pi as u32, ti as u32));
        }
    }
    let ranks = build_rank_bits(rows);
    occ.sort_by(|&(pa, ta), &(pb, tb)| {
        let sa = raws[pa as usize].tokens[ta as usize].as_bytes();
        let sb = raws[pb as usize].tokens[tb as usize].as_bytes();
        sa.cmp(sb)
            .then_with(|| ranks[pa as usize].cmp(&ranks[pb as usize]))
            .then_with(|| name_of(arena, rows, pa as usize).cmp(name_of(arena, rows, pb as usize)))
            .then_with(|| rows[pa as usize].id.cmp(&rows[pb as usize].id))
            .then((pa, ta).cmp(&(pb, tb)))
    });

    let mut unique_tokens: Vec<(u32, u16)> = Vec::new();
    let mut unique_token_masks: Vec<u64> = Vec::new();
    let mut occ_ids: Vec<u32> = Vec::with_capacity(occ.len());
    let mut tokens_sorted: Vec<(u32, u32)> = Vec::with_capacity(occ.len());
    let mut prev_span: Option<(u32, u16)> = None;

    for &(pi, ti) in &occ {
        let s = &raws[pi as usize].tokens[ti as usize];
        let is_dup = match prev_span {
            Some((off, len)) => &arena[off as usize..off as usize + len as usize] == s.as_str(),
            None => false,
        };
        if !is_dup {
            let (off, len) = push_span(arena, s);
            unique_tokens.push((off, len));
            unique_token_masks.push(byte_mask(s.as_bytes()));
            prev_span = Some((off, len));
        }
        let id = (unique_tokens.len() - 1) as u32;
        occ_ids.push(id);
        tokens_sorted.push((id, pi));
    }

    let mut starts: Vec<u32> = Vec::with_capacity(n);
    let mut acc: u32 = 0;
    for r in rows.iter() {
        starts.push(acc);
        acc += r.tokens_len as u32;
    }
    let mut cursor = starts.clone();
    let mut token_ids: Vec<u32> = vec![0u32; occ.len()];
    for (i, &(pi, _)) in occ.iter().enumerate() {
        token_ids[cursor[pi as usize] as usize] = occ_ids[i];
        cursor[pi as usize] += 1;
    }
    for (i, r) in rows.iter_mut().enumerate() {
        r.tokens_start = starts[i];
    }

    TokenInversion {
        token_ids,
        unique_tokens,
        unique_token_masks,
        tokens_sorted,
    }
}

fn name_of<'a>(arena: &'a str, rows: &[PkgRow], pi: usize) -> &'a str {
    let r = &rows[pi];
    &arena[r.name_off as usize..r.name_off as usize + r.name_len as usize]
}

fn build_max_kw_lens(raws: &[RawPkg]) -> Vec<u16> {
    raws.iter()
        .map(|raw| {
            raw.keywords
                .iter()
                .map(|k| k.len())
                .max()
                .unwrap_or(0)
                .try_into()
                .expect("keyword length exceeds u16")
        })
        .collect()
}

fn build_rank_bits(rows: &[PkgRow]) -> Vec<u64> {
    rows.iter()
        .map(|r| rank_bits(r.name_len, r.is_repo, r.popularity))
        .collect()
}

fn build_keyword_ids(
    raws: &[RawPkg],
    arena: &mut String,
    rows: &mut [PkgRow],
) -> (Vec<u32>, Vec<(u32, u16)>) {
    let mut kw_map: HashMap<&str, u32> = HashMap::new();
    let mut unique_kws: Vec<(u32, u16)> = Vec::new();
    let mut kw_ids: Vec<u32> = Vec::new();
    for (pi, raw) in raws.iter().enumerate() {
        rows[pi].kws_start = kw_ids.len() as u32;
        for k in &raw.keywords {
            let id = match kw_map.get(k.as_str()) {
                Some(&id) => id,
                None => {
                    let (off, len) = push_span(arena, k);
                    let id = unique_kws.len() as u32;
                    unique_kws.push((off, len));
                    kw_map.insert(k.as_str(), id);
                    id
                }
            };
            kw_ids.push(id);
        }
    }
    (kw_ids, unique_kws)
}

pub(crate) const CHAR_BITS: usize = 40;
pub(crate) const BIGRAM_BITS: usize = 64;
pub(crate) const TRIGRAM_BITS: usize = 64;

pub(crate) fn row_words(rows: usize) -> usize {
    rows.div_ceil(64)
}

fn build_word_table(masks: impl Iterator<Item = u64>, words: usize, bits: usize) -> Vec<u64> {
    let mut out = vec![0u64; words * bits];
    let keep = u64::MAX >> (64 - bits);
    for (i, mask) in masks.enumerate() {
        let mut rest = mask & keep;
        while rest != 0 {
            let bit = rest.trailing_zeros() as usize;
            out[word_of(i) * bits + bit] |= 1u64 << (i % 64);
            rest &= rest - 1;
        }
    }
    out
}

fn word_of(row: usize) -> usize {
    row / 64
}

struct TokenScan {
    ascii_ids: Vec<u32>,
    ascii_masks: Vec<u64>,
    ascii_starts: Vec<u32>,
    nonascii_ids: Vec<u32>,
}

fn token_is_ascii(arena: &str, unique_tokens: &[(u32, u16)], id: usize) -> bool {
    let (off, len) = unique_tokens[id];
    arena[off as usize..off as usize + len as usize].is_ascii()
}

fn build_token_scan(arena: &str, unique_tokens: &[(u32, u16)], masks: &[u64]) -> TokenScan {
    let (mut ascii_ids, nonascii_ids): (Vec<u32>, Vec<u32>) = (0..unique_tokens.len() as u32)
        .partition(|&id| token_is_ascii(arena, unique_tokens, id as usize));
    ascii_ids.sort_unstable_by_key(|&id| (unique_tokens[id as usize].1, id));
    let ascii_masks: Vec<u64> = ascii_ids.iter().map(|&id| masks[id as usize]).collect();
    let longest = ascii_ids
        .last()
        .map_or(0, |&id| unique_tokens[id as usize].1 as usize);
    let mut ascii_starts = vec![0u32; longest + 2];
    for &id in &ascii_ids {
        ascii_starts[unique_tokens[id as usize].1 as usize + 1] += 1;
    }
    for len in 1..ascii_starts.len() {
        ascii_starts[len] += ascii_starts[len - 1];
    }
    TokenScan {
        ascii_ids,
        ascii_masks,
        ascii_starts,
        nonascii_ids,
    }
}

pub(crate) fn assemble(raws: Vec<RawPkg>) -> PackageIndex {
    let n = raws.len();
    let mut arena: String = String::new();

    let mut rows = build_rows(&raws, &mut arena);
    let inversion = build_token_inversion(&raws, &mut arena, &mut rows);
    let (token_ids, unique_tokens, unique_token_masks, tokens_sorted) = (
        inversion.token_ids,
        inversion.unique_tokens,
        inversion.unique_token_masks,
        inversion.tokens_sorted,
    );
    let (kw_ids, unique_kws) = build_keyword_ids(&raws, &mut arena, &mut rows);
    let scan = build_token_scan(&arena, &unique_tokens, &unique_token_masks);
    let words = row_words(n);
    let name_char_words = build_word_table(rows.iter().map(|r| r.name_mask), words, CHAR_BITS);
    let kw_char_words = build_word_table(rows.iter().map(|r| r.kw_mask), words, CHAR_BITS);
    let name_bigram_words =
        build_word_table(rows.iter().map(|r| r.name_bigrams), words, BIGRAM_BITS);
    let kw_bigram_words = build_word_table(rows.iter().map(|r| r.kw_bigrams), words, BIGRAM_BITS);
    let name_trigram_words = build_word_table(
        raws.iter().map(|raw| trigram_mask(raw.name.as_bytes())),
        words,
        TRIGRAM_BITS,
    );
    let kw_trigram_words = build_word_table(
        raws.iter()
            .map(|raw| kw_ngram_mask(&raw.keywords, trigram_mask)),
        words,
        TRIGRAM_BITS,
    );

    let mut index = PackageIndex {
        rank_bits: build_rank_bits(&rows).into_boxed_slice(),
        max_kw_lens: build_max_kw_lens(&raws).into_boxed_slice(),
        rows,
        arena: arena.into_boxed_str(),
        token_ids: token_ids.into_boxed_slice(),
        kw_ids: kw_ids.into_boxed_slice(),
        unique_tokens: unique_tokens.into_boxed_slice(),
        unique_token_masks: unique_token_masks.into_boxed_slice(),
        token_scan_ids: scan.ascii_ids.into_boxed_slice(),
        token_scan_masks: scan.ascii_masks.into_boxed_slice(),
        token_scan_starts: scan.ascii_starts.into_boxed_slice(),
        token_scan_nonascii: scan.nonascii_ids.into_boxed_slice(),
        name_char_words: name_char_words.into_boxed_slice(),
        name_bigram_words: name_bigram_words.into_boxed_slice(),
        kw_bigram_words: kw_bigram_words.into_boxed_slice(),
        name_trigram_words: name_trigram_words.into_boxed_slice(),
        kw_trigram_words: kw_trigram_words.into_boxed_slice(),
        kw_char_words: kw_char_words.into_boxed_slice(),
        unique_kws: unique_kws.into_boxed_slice(),
        names_sorted: Vec::new(),
        tokens_sorted,
        version: 1,
        built_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };

    let mut names_sorted: Vec<u32> = (0..n as u32).collect();
    names_sorted.sort_by(|&a, &b| {
        index
            .name(a as usize)
            .as_bytes()
            .cmp(index.name(b as usize).as_bytes())
            .then(a.cmp(&b))
    });
    index.names_sorted = names_sorted;

    index
}

impl PackageIndex {
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn row(&self, pi: usize) -> &PkgRow {
        &self.rows[pi]
    }

    pub(crate) fn rank_word(&self, pi: usize) -> u64 {
        self.rank_bits[pi]
    }

    pub(crate) fn max_kw_len(&self, pi: usize) -> u16 {
        self.max_kw_lens[pi]
    }

    pub(crate) fn row_words(&self) -> usize {
        row_words(self.rows.len())
    }

    fn slice(&self, off: u32, len: u16) -> &str {
        &self.arena[off as usize..off as usize + len as usize]
    }

    pub(crate) fn name(&self, pi: usize) -> &str {
        let r = &self.rows[pi];
        self.slice(r.name_off, r.name_len)
    }

    pub(crate) fn norm_name(&self, pi: usize) -> &str {
        let r = &self.rows[pi];
        self.slice(r.norm_off, r.norm_len)
    }

    pub(crate) fn tokens_len(&self, pi: usize) -> usize {
        self.rows[pi].tokens_len as usize
    }

    pub(crate) fn token(&self, pi: usize, k: usize) -> &str {
        let start = self.rows[pi].tokens_start as usize;
        let id = self.token_ids[start + k];
        self.token_str(id as usize)
    }

    pub(crate) fn kws_len(&self, pi: usize) -> usize {
        self.rows[pi].kws_len as usize
    }

    pub(crate) fn keyword(&self, pi: usize, k: usize) -> &str {
        let start = self.rows[pi].kws_start as usize;
        let id = self.kw_ids[start + k];
        let (off, len) = self.unique_kws[id as usize];
        self.slice(off, len)
    }

    pub(crate) fn token_str(&self, id: usize) -> &str {
        let (off, len) = self.unique_tokens[id];
        self.slice(off, len)
    }

    pub(crate) fn token_len(&self, id: usize) -> usize {
        self.unique_tokens[id].1 as usize
    }

    pub(crate) fn token_bucket(&self, len: usize) -> Range<usize> {
        if len + 1 >= self.token_scan_starts.len() {
            return self.token_scan_ids.len()..self.token_scan_ids.len();
        }
        self.token_scan_starts[len] as usize..self.token_scan_starts[len + 1] as usize
    }

    pub(crate) fn name_char_word(&self, word: usize, bit: usize) -> u64 {
        self.name_char_words[word * CHAR_BITS + bit]
    }

    pub(crate) fn kw_char_word(&self, word: usize, bit: usize) -> u64 {
        self.kw_char_words[word * CHAR_BITS + bit]
    }

    pub(crate) fn name_bigram_word(&self, word: usize, bit: usize) -> u64 {
        self.name_bigram_words[word * BIGRAM_BITS + bit]
    }

    pub(crate) fn kw_bigram_word(&self, word: usize, bit: usize) -> u64 {
        self.kw_bigram_words[word * BIGRAM_BITS + bit]
    }

    pub(crate) fn name_trigram_word(&self, word: usize, bit: usize) -> u64 {
        self.name_trigram_words[word * TRIGRAM_BITS + bit]
    }

    pub(crate) fn kw_trigram_word(&self, word: usize, bit: usize) -> u64 {
        self.kw_trigram_words[word * TRIGRAM_BITS + bit]
    }

    pub fn exact_name_range(&self, q: &[u8]) -> Range<usize> {
        if q.is_empty() {
            return 0..0;
        }
        let lo = self
            .names_sorted
            .partition_point(|&i| self.name(i as usize).as_bytes() < q);
        let hi = self
            .names_sorted
            .partition_point(|&i| self.name(i as usize).as_bytes() <= q);
        lo..hi
    }

    pub fn prefix_name_range(&self, q: &[u8]) -> Range<usize> {
        if q.is_empty() {
            return 0..0;
        }
        let upper = next_prefix_bound(q);
        let lo = self
            .names_sorted
            .partition_point(|&i| self.name(i as usize).as_bytes() < q);
        let hi = match upper {
            None => self.names_sorted.len(),
            Some(u) => self
                .names_sorted
                .partition_point(|&i| self.name(i as usize).as_bytes() < u.as_slice()),
        };
        lo..hi
    }

    pub fn exact_token_range(&self, q: &[u8]) -> Range<usize> {
        if q.is_empty() {
            return 0..0;
        }
        let lo = self
            .tokens_sorted
            .partition_point(|&(tid, _)| self.token_str(tid as usize).as_bytes() < q);
        let hi = self
            .tokens_sorted
            .partition_point(|&(tid, _)| self.token_str(tid as usize).as_bytes() <= q);
        lo..hi
    }

    pub fn prefix_token_range(&self, q: &[u8]) -> Range<usize> {
        if q.is_empty() {
            return 0..0;
        }
        let upper = next_prefix_bound(q);
        let lo = self
            .tokens_sorted
            .partition_point(|&(tid, _)| self.token_str(tid as usize).as_bytes() < q);
        let hi = match upper {
            None => self.tokens_sorted.len(),
            Some(u) => self.tokens_sorted.partition_point(|&(tid, _)| {
                self.token_str(tid as usize).as_bytes() < u.as_slice()
            }),
        };
        lo..hi
    }

    pub fn store(&self, path: &Path, fingerprint: u64) -> Result<(), SearchError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let payload =
            bincode::serde::encode_to_vec((fingerprint, self), bincode::config::standard())
                .map_err(|e| SearchError::Encode(e.to_string()))?;
        let mut bytes = INDEX_MAGIC.to_vec();
        bytes.extend_from_slice(&payload);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<(PackageIndex, u64), SearchError> {
        let bytes = std::fs::read(path)?;
        if bytes.len() < INDEX_MAGIC.len() || bytes[..INDEX_MAGIC.len()] != INDEX_MAGIC {
            return Err(SearchError::Corrupt);
        }
        let ((fingerprint, index), _): ((u64, PackageIndex), _) =
            bincode::serde::decode_from_slice(
                &bytes[INDEX_MAGIC.len()..],
                bincode::config::standard(),
            )
            .map_err(|_| SearchError::Corrupt)?;
        if !index.validate() {
            return Err(SearchError::Corrupt);
        }
        Ok((index, fingerprint))
    }

    fn validate(&self) -> bool {
        let arena = &self.arena;
        let ok_span = |off: u32, len: u16| -> bool {
            let start = off as usize;
            let end = start + len as usize;
            end <= arena.len() && arena.is_char_boundary(start) && arena.is_char_boundary(end)
        };
        if self.rank_bits.len() != self.rows.len() || self.max_kw_lens.len() != self.rows.len() {
            return false;
        }
        if self.unique_token_masks.len() != self.unique_tokens.len() {
            return false;
        }
        if self.token_scan_ids.len() + self.token_scan_nonascii.len() != self.unique_tokens.len()
            || self.token_scan_masks.len() != self.token_scan_ids.len()
        {
            return false;
        }
        if self.token_scan_starts.last().copied() != Some(self.token_scan_ids.len() as u32) {
            return false;
        }
        for &id in &self.token_scan_nonascii {
            if id as usize >= self.unique_tokens.len() {
                return false;
            }
        }
        if self.name_char_words.len() != row_words(self.rows.len()) * CHAR_BITS
            || self.kw_char_words.len() != row_words(self.rows.len()) * CHAR_BITS
            || self.name_bigram_words.len() != row_words(self.rows.len()) * BIGRAM_BITS
            || self.kw_bigram_words.len() != row_words(self.rows.len()) * BIGRAM_BITS
            || self.name_trigram_words.len() != row_words(self.rows.len()) * TRIGRAM_BITS
            || self.kw_trigram_words.len() != row_words(self.rows.len()) * TRIGRAM_BITS
        {
            return false;
        }
        for slot in 0..self.token_scan_ids.len() {
            let id = self.token_scan_ids[slot] as usize;
            if id >= self.unique_tokens.len() {
                return false;
            }
            if self.token_scan_masks[slot] != self.unique_token_masks[id] {
                return false;
            }
        }
        for &(off, len) in &self.unique_tokens {
            if !ok_span(off, len) {
                return false;
            }
        }
        for &(off, len) in &self.unique_kws {
            if !ok_span(off, len) {
                return false;
            }
        }
        for r in &self.rows {
            if !ok_span(r.name_off, r.name_len) {
                return false;
            }
            if !ok_span(r.norm_off, r.norm_len) {
                return false;
            }
            if (r.tokens_start as usize + r.tokens_len as usize) > self.token_ids.len() {
                return false;
            }
            if (r.kws_start as usize + r.kws_len as usize) > self.kw_ids.len() {
                return false;
            }
            for k in 0..r.tokens_len as usize {
                let id = self.token_ids[r.tokens_start as usize + k];
                if id as usize >= self.unique_tokens.len() {
                    return false;
                }
            }
            for k in 0..r.kws_len as usize {
                let id = self.kw_ids[r.kws_start as usize + k];
                if id as usize >= self.unique_kws.len() {
                    return false;
                }
            }
        }
        for &pi in &self.names_sorted {
            if pi as usize >= self.rows.len() {
                return false;
            }
        }
        for &(tid, pi) in &self.tokens_sorted {
            if tid as usize >= self.unique_tokens.len() {
                return false;
            }
            if pi as usize >= self.rows.len() {
                return false;
            }
        }
        for w in self.tokens_sorted.windows(2) {
            if w[0].0 == w[1].0 && self.rank_bits[w[0].1 as usize] > self.rank_bits[w[1].1 as usize]
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // this is somehow a 5-10%ish performance penalty
    // fucking crazy
    #[test]
    fn row_stays_within_one_cache_line() {
        assert_eq!(std::mem::size_of::<PkgRow>(), 64);
    }

    #[test]
    fn every_mapped_char_fits_in_char_bits() {
        for b in 0u8..=255 {
            assert!(char_bit(b) >> CHAR_BITS == 0, "char_bit({b}) overflows");
        }
    }

    #[test]
    fn tokenize_splits_on_non_alphanumeric_runs() {
        assert_eq!(
            tokenize("google-chrome"),
            vec!["google".to_string(), "chrome".to_string()]
        );
        assert_eq!(
            tokenize("cava-git"),
            vec!["cava".to_string(), "git".to_string()]
        );
        assert_eq!(tokenize("g++"), vec!["g".to_string()]);
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn normalize_popularity_handles_none_repo_and_saturation() {
        assert_eq!(normalize_popularity(None, false), 0);
        assert_eq!(normalize_popularity(Some(50.0), true), 0);
        assert_eq!(normalize_popularity(Some(50.0), false), 32768);
        assert_eq!(normalize_popularity(Some(200.0), false), 65535);
    }

    #[test]
    fn parse_keywords_lowercases_and_splits() {
        assert!(parse_keywords(None).is_empty());
        assert_eq!(
            parse_keywords(Some("Foo Bar".to_string())),
            vec!["foo".to_string(), "bar".to_string()]
        );
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.bin");
        let raws = vec![
            RawPkg {
                id: 1,
                name: "vim".to_string(),
                tokens: vec!["vim".to_string()],
                keywords: vec![],
                popularity: 0,
                is_repo: false,
            },
            RawPkg {
                id: 2,
                name: "google-chrome".to_string(),
                tokens: vec!["google".to_string(), "chrome".to_string()],
                keywords: vec!["browser".to_string(), "web".to_string()],
                popularity: 100,
                is_repo: false,
            },
        ];
        let original = assemble(raws);
        original.store(&path, 7).expect("store");

        let (loaded, fingerprint) = PackageIndex::load(&path).expect("load");
        assert_eq!(fingerprint, 7);

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.name(0), "vim");
        assert_eq!(loaded.tokens_len(0), 1);
        assert_eq!(loaded.token(0, 0), "vim");
        assert_eq!(loaded.kws_len(0), 0);
        let r0 = loaded.row(0);
        assert_eq!(r0.id, 1);
        assert_eq!(r0.popularity, 0);
        assert!(!r0.is_repo);

        assert_eq!(loaded.name(1), "google-chrome");
        assert_eq!(loaded.tokens_len(1), 2);
        let mut toks: Vec<&str> = (0..loaded.tokens_len(1))
            .map(|k| loaded.token(1, k))
            .collect();
        toks.sort_unstable();
        assert_eq!(toks, vec!["chrome", "google"]);
        assert_eq!(loaded.kws_len(1), 2);
        let mut kws: Vec<&str> = (0..loaded.kws_len(1))
            .map(|k| loaded.keyword(1, k))
            .collect();
        kws.sort_unstable();
        assert_eq!(kws, vec!["browser", "web"]);
        let r1 = loaded.row(1);
        assert_eq!(r1.id, 2);
        assert_eq!(r1.popularity, 100);
        assert!(!r1.is_repo);

        let utoks: Vec<&str> = (0..loaded.unique_tokens.len())
            .map(|id| loaded.token_str(id))
            .collect();
        assert_eq!(utoks, vec!["chrome", "google", "vim"]);

        assert_eq!(loaded.names_sorted, vec![1, 0]);
        assert_eq!(loaded.exact_name_range(b"vim"), 1..2);

        let range = loaded.prefix_token_range(b"ch");
        let mut hit_pkgs = std::collections::HashSet::new();
        for i in range {
            hit_pkgs.insert(loaded.tokens_sorted[i].1);
        }
        assert_eq!(hit_pkgs.len(), 1);

        assert!(loaded.exact_name_range(b"nope").is_empty());
        assert!(loaded.prefix_token_range(b"zz").is_empty());
    }

    #[test]
    fn load_rejects_garbage_without_side_effects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.bin");
        std::fs::write(&path, b"not an index").expect("write garbage");
        let err = match PackageIndex::load(&path) {
            Ok(_) => panic!("garbage must fail"),
            Err(err) => err,
        };
        assert!(matches!(err, SearchError::Corrupt));
        assert!(path.exists(), "load must not delete the cache file");
    }
}
