use criterion::{Criterion, criterion_group, criterion_main};
use pakajo_search::engine::SearchEngine;
use pakajo_search::{IndexRow, SearchError, SearchFilter};
use std::collections::HashMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};

fn cache_db_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PAKAJO_CACHE_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("aur-meta.sqlite"));
    }
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => PathBuf::from(xdg),
        Err(_) => match std::env::var("HOME") {
            Ok(home) => PathBuf::from(home).join(".cache"),
            Err(_) => return None,
        },
    };
    Some(base.join("pakajo").join("aur-meta.sqlite"))
}

fn hash_entries(entries: &[(u128, u64)]) -> u64 {
    let mut hash = 14695981039346656037u64;
    for (nanos, len) in entries {
        for byte in nanos.to_le_bytes().iter().chain(len.to_le_bytes().iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1099511628211u64);
        }
    }
    hash
}

fn fingerprint_db(main: &Path) -> u64 {
    let mut paths = Vec::with_capacity(3);
    paths.push(main.to_path_buf());
    for suffix in ["-wal", "-shm"] {
        let mut name = std::ffi::OsString::from(main);
        name.push(suffix);
        paths.push(PathBuf::from(name));
    }
    let mut stats = Vec::with_capacity(3);
    for path in paths {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return u64::MAX,
        };
        let modified = match meta.modified() {
            Ok(modified) => modified,
            Err(_) => return u64::MAX,
        };
        let nanos = match modified.duration_since(std::time::UNIX_EPOCH) {
            Ok(age) => age.as_nanos(),
            Err(_) => return u64::MAX,
        };
        stats.push((nanos, meta.len()));
    }
    hash_entries(&stats)
}

fn load_rows(path: &Path) -> Result<Vec<IndexRow>, SearchError> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| SearchError::Store(Box::new(e)))?;
    let mut stmt = conn
        .prepare("SELECT rowid AS id, name, source, popularity, keywords FROM packages")
        .map_err(|e| SearchError::Store(Box::new(e)))?;
    let iter = stmt
        .query_map([], |row| {
            Ok(IndexRow {
                id: row.get(0)?,
                name: row.get(1)?,
                source: row.get(2)?,
                popularity: row.get(3)?,
                keywords: row.get(4)?,
            })
        })
        .map_err(|e| SearchError::Store(Box::new(e)))?;
    let mut rows = Vec::new();
    for row in iter {
        rows.push(row.map_err(|e| SearchError::Store(Box::new(e)))?);
    }
    Ok(rows)
}

fn live_engine() -> Option<(SearchEngine, tempfile::TempDir)> {
    let sqlite_path = match cache_db_path() {
        Some(path) => path,
        None => {
            eprintln!("skipping live bench: no cache directory");
            return None;
        }
    };
    let probe = match load_rows(&sqlite_path) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("skipping live bench: {e}");
            return None;
        }
    };
    if probe.is_empty() {
        eprintln!(
            "skipping live bench: catalog at {} is unpopulated",
            sqlite_path.display()
        );
        return None;
    }
    drop(probe);
    let fingerprint = fingerprint_db(&sqlite_path);
    let cache_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("skipping live bench: {e}");
            return None;
        }
    };
    let cache_path = cache_dir.path().join("index.bin");
    let owned_path = sqlite_path.clone();
    match SearchEngine::build(Some((cache_path.as_path(), fingerprint)), || {
        load_rows(&owned_path)
    }) {
        Ok(engine) => Some((engine, cache_dir)),
        Err(e) => {
            eprintln!("skipping live bench: {e}");
            None
        }
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    let Some((engine, _cache_dir)) = live_engine() else {
        return;
    };
    let queries = [
        "vi",
        "git",
        "linux",
        "python",
        "firefx",
        "c",
        "ch",
        "chr",
        "chro",
        "chroe",
        "chroem",
        "\"google chrome\"",
        "cosmic files",
        "google chrome",
        "gnome shell",
        "node js",
        "text editor",
    ];
    for q in queries {
        c.bench_function(&format!("search {q:?}"), |b| {
            b.iter(|| {
                let _ = engine
                    .query(black_box(q))
                    .filter(SearchFilter::All)
                    .execute(|_| Ok(HashMap::new()));
            });
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
