use criterion::{Criterion, criterion_group, criterion_main};
use pakajo::db::PackageDb;
use pakajo::search::engine::SearchEngine;
use std::hint::black_box;

const MIN_LIVE_PACKAGES: i64 = 1_000;

fn live_engine() -> SearchEngine {
    let sqlite_path = PackageDb::db_path().expect("resolve pakajo cache db path");
    let count = rusqlite::Connection::open_with_flags(
        &sqlite_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .and_then(|conn| {
        conn.query_row("SELECT COUNT(*) FROM packages", [], |row| {
            row.get::<_, i64>(0)
        })
    })
    .expect("read package count from pakajo cache db");

    assert!(
        count >= MIN_LIVE_PACKAGES,
        "cache db at {} has only {count} packages, expected >= {MIN_LIVE_PACKAGES}",
        sqlite_path.display()
    );

    SearchEngine::new(sqlite_path).expect("build search engine")
}

fn criterion_benchmark(c: &mut Criterion) {
    let engine = live_engine();
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
    ];

    for q in queries {
        c.bench_function(&format!("search {q:?}"), |b| {
            b.iter(|| engine.search_tiered(black_box(q)));
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
