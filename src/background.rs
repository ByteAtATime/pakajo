use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures::FutureExt as _;
use futures::SinkExt as _;
use futures::StreamExt as _;

use cosmic::iced::Subscription;
use cosmic::iced::stream::channel;
use pakajo::db::{AUR_SYNC_MIN_INTERVAL, PackageDb, RefreshOutcome};
use pakajo::pacman::init_alpm;
use pakajo::search::engine::SearchEngine;

pub const LOCK_DEBOUNCE: Duration = Duration::from_millis(300);

pub fn begin_aur_sync_in_background(db: Arc<PackageDb>, search_engine: Option<Arc<SearchEngine>>) {
    std::thread::spawn(move || {
        let reniced = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
        if reniced != 0 {
            eprintln!(
                "[pakajo] failed to renice aur sync worker: {}",
                std::io::Error::last_os_error()
            );
        }

        if let Some(age) = db.last_refreshed_age()
            && age < AUR_SYNC_MIN_INTERVAL
        {
            eprintln!(
                "[pakajo] skipping aur sync (last refresh {}h ago)",
                age.as_secs() / 3600
            );
            return;
        }

        let handle = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed to init alpm: {e:#}");
                return;
            }
        };

        match db.refresh(&handle) {
            Ok(RefreshOutcome::NotModified) => {
                eprintln!("[pakajo] aur index up to date");
            }
            Ok(RefreshOutcome::Updated {
                aur_count,
                repo_count,
                skipped,
            }) => {
                eprintln!(
                    "[pakajo] indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})"
                );
                if let Some(engine) = search_engine.as_ref() {
                    let _ = engine.ensure_fresh();
                }
            }
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed: {e:#}");
            }
        }
    });
}

struct DbLockWatcher;

pub(crate) fn db_lock_watcher_subscription() -> Subscription<crate::Message> {
    Subscription::run_with(std::any::TypeId::of::<DbLockWatcher>(), |_| {
        channel(
            16,
            |mut tx: futures::channel::mpsc::Sender<crate::Message>| async move {
                let (wtx, mut wrx) = futures::channel::mpsc::channel::<()>(16);

                let db_dir = pacmanconf::Config::new()
                    .ok()
                    .map(|c| std::path::PathBuf::from(c.db_path))
                    .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/pacman"));

                pakajo::pacman_watch::spawn_db_lock_watcher(db_dir, wtx);

                while let Some(()) = wrx.next().await {
                    while wrx.next().now_or_never().is_some() {}
                    let (stx, srx) = futures::channel::oneshot::channel::<()>();
                    std::thread::spawn(move || {
                        std::thread::sleep(LOCK_DEBOUNCE);
                        let _ = stx.send(());
                    });
                    let _ = srx.await;
                    if tx.send(crate::Message::DbLockReleased).await.is_err() {
                        break;
                    }
                }
            },
        )
    })
}
