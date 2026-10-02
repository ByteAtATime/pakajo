use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::SinkExt as _;
use futures::StreamExt as _;

use cosmic::iced::Subscription;
use cosmic::iced::stream::channel;
use pakajo::db::SearchSession;
use pakajo::db::{AUR_SYNC_MIN_INTERVAL, PackageDb, RefreshOutcome};
use pakajo::pacman::handle;

pub const LOCK_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Debug)]
pub enum IndexSyncOutcome {
    Skipped,
    NotModified,
    Updated,
    Failed(String),
}

pub fn aur_sync_task(
    db: Arc<PackageDb>,
    search: Option<Arc<SearchSession>>,
) -> cosmic::app::Task<crate::Message> {
    let (tx, rx) = futures::channel::oneshot::channel::<IndexSyncOutcome>();
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
            let _ = tx.send(IndexSyncOutcome::Skipped);
            return;
        }

        let handle = match handle() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed to init alpm: {e:#}");
                let _ = tx.send(IndexSyncOutcome::Failed(format!("{e:#}")));
                return;
            }
        };

        match db.refresh(&handle) {
            Ok(RefreshOutcome::NotModified) => {
                eprintln!("[pakajo] aur index up to date");
                let _ = tx.send(IndexSyncOutcome::NotModified);
            }
            Ok(RefreshOutcome::Updated {
                aur_count,
                repo_count,
                skipped,
            }) => {
                eprintln!(
                    "[pakajo] indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})"
                );
                if let Some(search) = search.as_ref()
                    && let Err(e) = search.rebuild()
                {
                    eprintln!("[pakajo] search index rebuild failed: {e}");
                }
                let _ = tx.send(IndexSyncOutcome::Updated);
            }
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed: {e:#}");
                let _ = tx.send(IndexSyncOutcome::Failed(format!("{e:#}")));
            }
        }
    });
    cosmic::app::Task::perform(
        async move {
            rx.await.unwrap_or_else(|_| {
                IndexSyncOutcome::Failed("aur sync worker exited without reporting".to_string())
            })
        },
        |outcome| {
            crate::Message::Loading(crate::components::loading::LoadingMessage::Synced(outcome))
                .into()
        },
    )
}

struct DbLockWatcher;

pub(crate) fn db_lock_watcher_subscription() -> Subscription<crate::Message> {
    Subscription::run_with(std::any::TypeId::of::<DbLockWatcher>(), |_| {
        channel(
            16,
            |mut tx: futures::channel::mpsc::Sender<crate::Message>| async move {
                let (wtx, mut wrx) = futures::channel::mpsc::channel::<()>(16);

                let db_dir = pakajo::pacman::db_path();

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
