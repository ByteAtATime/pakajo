use criterion::{Criterion, criterion_group, criterion_main};
use pakajo_search::engine::SearchEngine;
use pakajo_search::{IndexRow, SearchError, SearchFilter};
use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;

fn catalog_path() -> Option<PathBuf> {
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

fn load_rows(path: &std::path::Path) -> Result<Vec<IndexRow>, SearchError> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(store)?;
    let mut stmt = conn
        .prepare("SELECT rowid AS id, name, source, popularity, keywords FROM packages")
        .map_err(store)?;
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
        .map_err(store)?;
    let mut rows = Vec::new();
    for row in iter {
        rows.push(row.map_err(store)?);
    }
    Ok(rows)
}

fn store(e: rusqlite::Error) -> SearchError {
    SearchError::Store(Box::new(e))
}

fn live_engine() -> Option<SearchEngine> {
    let path = catalog_path()?;
    match load_rows(&path) {
        Ok(rows) if !rows.is_empty() => {}
        Ok(_) => {
            eprintln!(
                "skipping live bench: catalog at {} is unpopulated",
                path.display()
            );
            return None;
        }
        Err(e) => {
            eprintln!("skipping live bench: {e}");
            return None;
        }
    }
    match SearchEngine::build(None, || load_rows(&path)) {
        Ok(engine) => Some(engine),
        Err(e) => {
            eprintln!("skipping live bench: {e}");
            None
        }
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    let Some(engine) = live_engine() else {
        return;
    };
    for q in [
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
    ] {
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
