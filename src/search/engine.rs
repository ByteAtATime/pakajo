use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::search::fuzzy::typo_tier;
use crate::search::index::{needs_rebuild, PackageIndex};
use crate::search::query::{parse_query, ParsedQuery};
use crate::search::tiers::{best_concrete_tier, candidate_ordering, Candidate, Tier};

const RESULT_LIMIT: usize = 30;
const TYPO_GATE: usize = 5;

const SHORT_TIERS: &[Tier] = &[Tier::ExactName, Tier::ExactToken, Tier::PrefixName];
const ALL_CONCRETE_TIERS: &[Tier] = &[
    Tier::ExactName,
    Tier::ExactToken,
    Tier::PrefixName,
    Tier::PrefixToken,
    Tier::Substring,
    Tier::Keyword,
];

pub fn search_index(index: &PackageIndex, text: &str) -> Vec<u32> {
    let Some(pq) = parse_query(text) else {
        return Vec::new();
    };
    let q = pq.text();
    match &pq {
        ParsedQuery::Quoted(_) => quoted_ids(index, q),
        ParsedQuery::Short(_) => to_sorted_ids(concrete_in(index, q, SHORT_TIERS)),
        ParsedQuery::Normal(_) => {
            let cands = concrete_in(index, q, ALL_CONCRETE_TIERS);
            if cands.len() >= TYPO_GATE {
                return to_sorted_ids(cands);
            }
            to_sorted_ids(append_typo_fallback(index, q, cands))
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

fn concrete_in<'a>(index: &'a PackageIndex, q: &str, allowed: &[Tier]) -> Vec<Candidate<'a>> {
    index
        .packages
        .iter()
        .filter_map(|p| match best_concrete_tier(p, q) {
            Some(tier) if allowed.contains(&tier) => Some(Candidate { pkg: p, tier }),
            _ => None,
        })
        .collect()
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

fn append_typo_fallback<'a>(
    index: &'a PackageIndex,
    q: &str,
    mut cands: Vec<Candidate<'a>>,
) -> Vec<Candidate<'a>> {
    let present: HashSet<u32> = cands.iter().map(|c| c.pkg.id).collect();
    for p in &index.packages {
        if present.contains(&p.id) {
            continue;
        }
        if typo_tier(p, q).is_some() {
            cands.push(Candidate {
                pkg: p,
                tier: Tier::Typo,
            });
        }
    }
    cands
}

fn to_sorted_ids(mut cands: Vec<Candidate>) -> Vec<u32> {
    cands.sort_by(candidate_ordering);
    cands.truncate(RESULT_LIMIT);
    cands.iter().map(|c| c.pkg.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::{tokenize, IndexedPackage};

    fn pkg(id: u32, name: &str, is_repo: bool, popularity: u16) -> IndexedPackage {
        IndexedPackage {
            id,
            name: name.to_string(),
            tokens: tokenize(name),
            keywords: Vec::new(),
            popularity,
            is_repo,
        }
    }

    fn index_with(packages: Vec<IndexedPackage>) -> PackageIndex {
        PackageIndex {
            packages,
            version: 1,
            built_at: 0,
        }
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
