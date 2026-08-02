use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const POP_NORM_MAX: f64 = 100.0;
const INDEX_MAGIC: [u8; 4] = *b"PKJ1";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct IndexedPackage {
    pub id: u32,
    pub name: String,
    pub tokens: Vec<String>,
    pub keywords: Vec<String>,
    pub popularity: u16,
    pub is_repo: bool,
    pub name_mask: u64,
    pub kw_mask: u64,
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

pub struct IndexRow {
    pub id: u32,
    pub name: String,
    pub source: String,
    pub popularity: Option<f64>,
    pub keywords: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PackageIndex {
    pub packages: Vec<IndexedPackage>,
    pub version: u32,
    pub built_at: u64,
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
        Some(s) => s
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect(),
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
    let packages = rows
        .into_iter()
        .map(|row| {
            let is_repo = row.source == "repo";
            let name = row.name.to_lowercase();
            let tokens = tokenize(&row.name);
            let keywords = parse_keywords(row.keywords);
            let name_mask = byte_mask(name.as_bytes());
            let kw_mask = keywords
                .iter()
                .map(|k| byte_mask(k.as_bytes()))
                .fold(0u64, |acc, m| acc | m);
            IndexedPackage {
                id: row.id,
                name,
                tokens,
                keywords,
                popularity: normalize_popularity(row.popularity, is_repo),
                is_repo,
                name_mask,
                kw_mask,
            }
        })
        .collect();
    let built_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PackageIndex {
        packages,
        version: 1,
        built_at,
    }
}

impl PackageIndex {
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
            Ok((pkg, _)) => Ok(pkg),
            Err(e) => {
                let _ = std::fs::remove_file(path);
                Err(anyhow::Error::from(e))
            }
        }
    }

    pub fn load_or_build(sqlite_path: &Path) -> anyhow::Result<PackageIndex> {
        let idx = index_path(sqlite_path);
        if !needs_rebuild(sqlite_path) {
            if let Ok(pkg) = Self::load(&idx) {
                return Ok(pkg);
            }
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
        let original = PackageIndex {
            packages: vec![IndexedPackage {
                id: 7,
                name: "google-chrome".to_string(),
                tokens: vec!["google".to_string(), "chrome".to_string()],
                keywords: vec!["browser".to_string()],
                popularity: 32768,
                is_repo: false,
                name_mask: byte_mask("google-chrome".as_bytes()),
                kw_mask: byte_mask("browser".as_bytes()),
            }],
            version: 1,
            built_at: 12345,
        };
        original.save(&path).expect("save");

        let loaded = PackageIndex::load(&path).expect("load");

        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.built_at, original.built_at);
        assert_eq!(loaded.packages.len(), 1);
        let got = &loaded.packages[0];
        let want = &original.packages[0];
        assert_eq!(got.id, want.id);
        assert_eq!(got.name, want.name);
        assert_eq!(got.tokens, want.tokens);
        assert_eq!(got.keywords, want.keywords);
        assert_eq!(got.popularity, want.popularity);
        assert_eq!(got.is_repo, want.is_repo);
        assert_eq!(got.name_mask, want.name_mask);
        assert_eq!(got.kw_mask, want.kw_mask);
    }
}
