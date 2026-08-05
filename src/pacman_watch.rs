use futures::channel::mpsc;
use inotify::{EventMask, Inotify, WatchMask};
use std::ffi::OsStr;
use std::path::Path;
use std::thread;

pub fn spawn_db_lock_watcher(db_dir: impl AsRef<Path>, mut tx: mpsc::Sender<()>) {
    let db_dir = db_dir.as_ref().to_path_buf();
    thread::spawn(move || {
        let mut inotify = match Inotify::init() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("[pakajo] db.lck watcher: inotify init failed: {e}");
                return;
            }
        };
        if let Err(e) = inotify
            .watches()
            .add(&db_dir, WatchMask::DELETE | WatchMask::MOVED_FROM)
        {
            eprintln!(
                "[pakajo] db.lck watcher: cannot watch {}: {e}",
                db_dir.display()
            );
            return;
        }
        let target = OsStr::new("db.lck");
        let mut buffer = [0u8; 4096];
        'watcher: loop {
            let events = match inotify.read_events_blocking(&mut buffer) {
                Ok(ev) => ev,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break 'watcher,
            };
            let mut released = false;
            let mut evicted = false;
            for ev in events {
                if ev.mask.contains(EventMask::IGNORED) {
                    evicted = true;
                    continue;
                }
                if ev.name == Some(target)
                    && (ev.mask.contains(EventMask::DELETE)
                        || ev.mask.contains(EventMask::MOVED_FROM))
                {
                    released = true;
                }
            }
            if evicted {
                eprintln!("[pakajo] db.lck watcher: watch evicted (IN_IGNORED), stopping");
                break 'watcher;
            }
            if released {
                loop {
                    match tx.try_send(()) {
                        Ok(()) => break,
                        Err(err) => {
                            if err.is_disconnected() {
                                break 'watcher;
                            }
                            std::thread::yield_now();
                        }
                    }
                }
            }
        }
    });
}
