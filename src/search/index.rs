use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const POP_NORM_MAX: f64 = 100.0;
const INDEX_MAGIC: [u8; 4] = *b"PKJ2";

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
    name_len: u16,
    tokens_start: u32,
    tokens_len: u16,
    kws_start: u32,
    kws_len: u16,
    pub(crate) popularity: u16,
    pub(crate) is_repo: bool,
    pub(crate) name_mask: u64,
    pub(crate) kw_mask: u64,
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
    pub(crate) arena: Box<str>,
    pub(crate) token_ids: Box<[u32]>,
    pub(crate) kw_ids: Box<[u32]>,
    pub(crate) unique_tokens: Box<[(u32, u16)]>,
    pub(crate) unique_token_masks: Box<[u64]>,
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

pub fn index_path(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_file_name("index.bin")
}

pub fn needs_rebuild(sqlite_path: &Path) -> bool {
    let idx = index_path(sqlite_path);
    let Ok(idx_meta) = std::fs::metadata(&idx) else {
        return true;
    };
    let Ok(sqlite_meta) = std::fs::metadata(sqlite_path) else {
        return true;
    };
    let Ok(idx_mod) = idx_meta.modified() else {
        return true;
    };
    let Ok(sqlite_mod) = sqlite_meta.modified() else {
        return true;
    };
    idx_mod < sqlite_mod
}

pub struct IndexRow {
    pub id: u32,
    pub name: String,
    pub source: String,
    pub popularity: Option<f64>,
    pub keywords: Option<String>,
}

fn scan_packages(conn: &rusqlite::Connection) -> anyhow::Result<Vec<IndexRow>> {
    let mut stmt =
        conn.prepare("SELECT rowid AS id, name, source, popularity, keywords FROM packages")?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        Ok(IndexRow {
            id: id as u32,
            name: row.get(1)?,
            source: row.get(2)?,
            popularity: row.get(3)?,
            keywords: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<IndexRow>>>()
        .map_err(anyhow::Error::from)
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

fn build_rows(raws: &[RawPkg], arena: &mut String) -> Vec<PkgRow> {
    let n = raws.len();
    let mut rows: Vec<PkgRow> = Vec::with_capacity(n);
    for raw in raws {
        let (name_off, name_len) = push_span(arena, &raw.name);
        let kw_mask = raw
            .keywords
            .iter()
            .map(|k| byte_mask(k.as_bytes()))
            .fold(0u64, |acc, m| acc | m);
        let name_mask = byte_mask(raw.name.as_bytes());
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
            tokens_start: 0,
            tokens_len,
            kws_start: 0,
            kws_len,
            popularity: raw.popularity,
            is_repo: raw.is_repo,
            name_mask,
            kw_mask,
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
    occ.sort_by(|&(pa, ta), &(pb, tb)| {
        let sa = raws[pa as usize].tokens[ta as usize].as_bytes();
        let sb = raws[pb as usize].tokens[tb as usize].as_bytes();
        sa.cmp(sb).then((pa, ta).cmp(&(pb, tb)))
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

    let mut index = PackageIndex {
        rows,
        arena: arena.into_boxed_str(),
        token_ids: token_ids.into_boxed_slice(),
        kw_ids: kw_ids.into_boxed_slice(),
        unique_tokens: unique_tokens.into_boxed_slice(),
        unique_token_masks: unique_token_masks.into_boxed_slice(),
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

    fn slice(&self, off: u32, len: u16) -> &str {
        &self.arena[off as usize..off as usize + len as usize]
    }

    pub(crate) fn name(&self, pi: usize) -> &str {
        let r = &self.rows[pi];
        self.slice(r.name_off, r.name_len)
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

    pub(crate) fn view(&self, pi: usize) -> crate::search::tiers::PkgView<'_> {
        let r = &self.rows[pi];
        crate::search::tiers::PkgView {
            name: self.name(pi),
            id: r.id,
            popularity: r.popularity,
            is_repo: r.is_repo,
        }
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

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut bytes = INDEX_MAGIC.to_vec();
        bytes.extend_from_slice(&bincode::serde::encode_to_vec(
            self,
            bincode::config::standard(),
        )?);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<PackageIndex> {
        let bytes = std::fs::read(path)?;
        if bytes.len() < INDEX_MAGIC.len() || bytes[..INDEX_MAGIC.len()] != INDEX_MAGIC {
            let _ = std::fs::remove_file(path);
            anyhow::bail!("index magic mismatch");
        }
        match bincode::serde::decode_from_slice::<PackageIndex, _>(
            &bytes[INDEX_MAGIC.len()..],
            bincode::config::standard(),
        ) {
            Ok((pkg, _)) => {
                if !pkg.validate() {
                    let _ = std::fs::remove_file(path);
                    anyhow::bail!("index validation failed");
                }
                Ok(pkg)
            }
            Err(e) => {
                let _ = std::fs::remove_file(path);
                Err(anyhow::Error::from(e))
            }
        }
    }

    pub fn load_or_build(sqlite_path: &Path) -> anyhow::Result<PackageIndex> {
        let idx = index_path(sqlite_path);
        if !needs_rebuild(sqlite_path)
            && let Ok(pkg) = Self::load(&idx)
        {
            return Ok(pkg);
        }
        let rows = match rusqlite::Connection::open_with_flags(
            sqlite_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(conn) => scan_packages(&conn).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let pkg = build_from_rows(rows);
        let _ = pkg.save(&idx);
        Ok(pkg)
    }

    fn validate(&self) -> bool {
        let arena = &self.arena;
        let ok_span = |off: u32, len: u16| -> bool {
            let start = off as usize;
            let end = start + len as usize;
            end <= arena.len() && arena.is_char_boundary(start) && arena.is_char_boundary(end)
        };
        if self.unique_token_masks.len() != self.unique_tokens.len() {
            return false;
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        original.save(&path).expect("save");

        let loaded = PackageIndex::load(&path).expect("load");

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.name(0), "vim");
        assert_eq!(loaded.tokens_len(0), 1);
        assert_eq!(loaded.token(0, 0), "vim");
        assert_eq!(loaded.kws_len(0), 0);
        let v0 = loaded.view(0);
        assert_eq!(v0.id, 1);
        assert_eq!(v0.popularity, 0);
        assert!(!v0.is_repo);

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
        let v1 = loaded.view(1);
        assert_eq!(v1.id, 2);
        assert_eq!(v1.popularity, 100);
        assert!(!v1.is_repo);

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
}
