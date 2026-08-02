use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::search::fuzzy::{TypoMatcher, MAX_EDIT_DISTANCE};
use crate::search::index::{byte_mask, needs_rebuild, PackageIndex};
use crate::search::query::{parse_query, ParsedQuery};
use crate::search::tiers::{best_concrete_tier_in, candidate_ordering, Candidate, Tier};

const RESULT_LIMIT: usize = 30;
const TYPO_GATE: usize = 5;

const SHORT_TIERS: &[Tier] = &[Tier::ExactName, Tier::ExactToken, Tier::PrefixName];
const CHEAP_TIERS_ALL: &[Tier] = &[
    Tier::ExactName,
    Tier::ExactToken,
    Tier::PrefixName,
    Tier::PrefixToken,
];
const EXPENSIVE_TIERS: &[Tier] = &[Tier::Substring, Tier::Keyword];

pub fn search_index(index: &PackageIndex, text: &str) -> Vec<u32> {
    let Some(pq) = parse_query(text) else {
        return Vec::new();
    };
    let q = pq.text();
    match &pq {
        ParsedQuery::Quoted(_) => quoted_ids(index, q),
        ParsedQuery::Short(_) => to_sorted_ids(gather_cheap_candidates(index, q, SHORT_TIERS)),
        ParsedQuery::Normal(_) => {
            let cheap = gather_cheap_candidates(index, q, CHEAP_TIERS_ALL);
            if cheap.len() >= RESULT_LIMIT {
                return to_sorted_ids(cheap);
            }
            let seen: HashSet<u32> = cheap.iter().map(|c| c.pkg.id).collect();
            let cands = if cheap.len() >= TYPO_GATE {
                expensive_only_pass(index, q, cheap, &seen)
            } else {
                fused_expensive_typo_pass(index, q, cheap, &seen)
            };
            to_sorted_ids(cands)
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

    pub fn search(&self, text: &str) -> Vec<u32> {
        let snapshot = self.index.read().expect("index lock poisoned").clone();
        search_index(&snapshot, text)
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

fn gather_cheap_candidates<'a>(
    index: &'a PackageIndex,
    q: &str,
    allowed: &[Tier],
) -> Vec<Candidate<'a>> {
    let qmask = byte_mask(q.as_bytes());
    let q_bytes = q.as_bytes();
    let mut idxs: Vec<u32> = Vec::new();
    if allowed.contains(&Tier::ExactName) {
        for i in index.exact_name_range(q_bytes) {
            idxs.push(index.names_sorted[i]);
        }
    }
    if allowed.contains(&Tier::ExactToken) {
        for i in index.exact_token_range(q_bytes) {
            idxs.push(index.tokens_sorted[i].0);
        }
    }
    if allowed.contains(&Tier::PrefixName) {
        for i in index.prefix_name_range(q_bytes) {
            idxs.push(index.names_sorted[i]);
        }
    }
    if allowed.contains(&Tier::PrefixToken) {
        for i in index.prefix_token_range(q_bytes) {
            idxs.push(index.tokens_sorted[i].0);
        }
    }
    idxs.sort_unstable();
    idxs.dedup();
    let mut cands: Vec<Candidate<'a>> = Vec::with_capacity(idxs.len());
    for i in idxs {
        let p = &index.packages[i as usize];
        if let Some(tier) = best_concrete_tier_in(p, q, allowed, qmask) {
            cands.push(Candidate { pkg: p, tier });
        }
    }
    cands
}

fn expensive_only_pass<'a>(
    index: &'a PackageIndex,
    q: &str,
    mut cands: Vec<Candidate<'a>>,
    seen: &HashSet<u32>,
) -> Vec<Candidate<'a>> {
    let qmask = byte_mask(q.as_bytes());
    for p in &index.packages {
        if seen.contains(&p.id) {
            continue;
        }
        if let Some(tier) = best_concrete_tier_in(p, q, EXPENSIVE_TIERS, qmask) {
            cands.push(Candidate { pkg: p, tier });
        }
    }
    cands
}

fn fused_expensive_typo_pass<'a>(
    index: &'a PackageIndex,
    q: &str,
    mut cands: Vec<Candidate<'a>>,
    seen: &HashSet<u32>,
) -> Vec<Candidate<'a>> {
    let qmask = byte_mask(q.as_bytes());
    let q_ascii = q.is_ascii();
    let mut matcher = TypoMatcher::new(q.as_bytes());
    let mut typo_buf: Vec<Candidate<'a>> = Vec::new();
    let mut placed: HashSet<u32> = HashSet::new();

    for (i, p) in index.packages.iter().enumerate() {
        let idx = i as u32;
        if seen.contains(&p.id) {
            placed.insert(idx);
            continue;
        }
        let name_missing = qmask & !p.name_mask;
        if (name_missing == 0 || (qmask & !p.kw_mask) == 0)
            && let Some(tier) = best_concrete_tier_in(p, q, EXPENSIVE_TIERS, qmask)
        {
            cands.push(Candidate { pkg: p, tier });
            placed.insert(idx);
            continue;
        }
        if name_missing.count_ones() as usize > MAX_EDIT_DISTANCE {
            continue;
        }
        let name_len_ok = !(q_ascii && p.name.is_ascii())
            || p.name.len().abs_diff(q.len()) <= MAX_EDIT_DISTANCE;
        if name_len_ok && matcher.within(p.name.as_bytes(), p.name_mask, MAX_EDIT_DISTANCE) {
            typo_buf.push(Candidate { pkg: p, tier: Tier::Typo });
            placed.insert(idx);
        }
    }

    for (token, token_mask) in &index.unique_tokens {
        if (qmask & !token_mask).count_ones() as usize > MAX_EDIT_DISTANCE {
            continue;
        }
        if q_ascii && token.is_ascii() && token.len().abs_diff(q.len()) > MAX_EDIT_DISTANCE {
            continue;
        }
        if !matcher.within(token.as_bytes(), *token_mask, MAX_EDIT_DISTANCE) {
            continue;
        }
        for i in index.exact_token_range(token.as_bytes()) {
            let pkg_idx = index.tokens_sorted[i].0;
            if placed.contains(&pkg_idx) {
                continue;
            }
            typo_buf.push(Candidate {
                pkg: &index.packages[pkg_idx as usize],
                tier: Tier::Typo,
            });
            placed.insert(pkg_idx);
        }
    }

    if cands.len() < TYPO_GATE {
        cands.append(&mut typo_buf);
    }
    cands
}

fn quoted_ids(index: &PackageIndex, q: &str) -> Vec<u32> {
    let cands = index
        .packages
        .iter()
        .filter(|p| p.name.contains(q))
        .map(|p| Candidate {
            pkg: p,
            tier: Tier::Substring,
        })
        .collect();
    to_sorted_ids(cands)
}

fn to_sorted_ids(mut cands: Vec<Candidate>) -> Vec<u32> {
    if cands.len() > RESULT_LIMIT {
        cands.select_nth_unstable_by(RESULT_LIMIT, candidate_ordering);
        cands[..RESULT_LIMIT + 1].sort_by(candidate_ordering);
    } else {
        cands.sort_by(candidate_ordering);
    }
    cands.truncate(RESULT_LIMIT);
    cands.iter().map(|c| c.pkg.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::{tokenize, IndexedPackage};

    fn pkg(id: u32, name: &str, is_repo: bool, popularity: u16) -> IndexedPackage {
        let tokens = tokenize(name);
        let name_mask = byte_mask(name.as_bytes());
        IndexedPackage {
            id,
            name: name.to_string(),
            tokens,
            keywords: Vec::new(),
            popularity,
            is_repo,
            name_mask,
            kw_mask: 0,
        }
    }

    fn index_with(packages: Vec<IndexedPackage>) -> PackageIndex {
        let mut index = PackageIndex {
            packages,
            names_sorted: Vec::new(),
            tokens_sorted: Vec::new(),
            unique_tokens: Vec::new(),
            version: 1,
            built_at: 0,
        };
        index.build_inverted();
        index
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

        let exact_name = gather_cheap_candidates(&index, "vim", &[Tier::ExactName]);
        assert_eq!(ids_of(&exact_name), vec![1]);

        let exact_token = gather_cheap_candidates(&index, "bin", &[Tier::ExactToken]);
        assert_eq!(ids_of(&exact_token), vec![3]);

        let prefix_name = gather_cheap_candidates(&index, "vim", &[Tier::PrefixName]);
        assert_eq!(ids_of(&prefix_name), vec![1, 2]);

        let prefix_token = gather_cheap_candidates(&index, "vi", &[Tier::PrefixToken]);
        assert_eq!(ids_of(&prefix_token), vec![1, 2, 5]);

        let all_cheap = gather_cheap_candidates(&index, "vim", CHEAP_TIERS_ALL);
        assert_eq!(ids_of(&all_cheap), vec![1, 2, 5]);
        assert!(all_cheap.iter().all(|c| c.tier != Tier::Substring));
    }

    fn ids_of(cands: &[Candidate]) -> Vec<u32> {
        let mut v: Vec<u32> = cands.iter().map(|c| c.pkg.id).collect();
        v.sort();
        v
    }

    #[test]
    fn empty_query_returns_empty() {
        let index = index_with(vec![pkg(1, "vim", false, 0)]);
        assert!(search_index(&index, "").is_empty());
        assert!(search_index(&index, "   ").is_empty());
    }

    #[test]
    fn exact_name_ranks_first() {
        let index = index_with(vec![pkg(1, "vim", false, 0), pkg(2, "vim-plugins", false, 0)]);
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
    fn normal_typo_fallback_runs_when_concrete_sparse() {
        let index = index_with(vec![pkg(1, "google-chrome", false, 0)]);
        let ids = search_index(&index, "chroem");
        assert!(ids.contains(&1));
    }

    #[test]
    fn normal_typo_skipped_when_concrete_plentiful() {
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
        let packages: Vec<IndexedPackage> = (1u32..=40)
            .map(|i| pkg(i, &format!("prefix-{i}"), false, 0))
            .collect();
        let index = index_with(packages);
        let ids = search_index(&index, "prefix");
        assert_eq!(ids.len(), 30);
    }

    #[test]
    fn truncation_excludes_substring_only_when_limit_full() {
        let mut packages: Vec<IndexedPackage> = (1u32..=RESULT_LIMIT as u32)
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
}

#[cfg(test)]
mod search_engine_tests {
    use super::*;
    use crate::local_index::LocalIndex;
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
        let _local = LocalIndex::open(&sqlite_path).expect("open");
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
        let _local = LocalIndex::open(&sqlite_path).expect("open");
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
        let _local = LocalIndex::open(&sqlite_path).expect("open");
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
