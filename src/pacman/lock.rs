use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use alpm_sys::alpm_handle_t;

static COMMIT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static INTERRUPT_SIGNAL: AtomicI32 = AtomicI32::new(0);
static WAITING_FOR_LOCK: AtomicBool = AtomicBool::new(false);

pub const LOCK_POLL_INTERVAL: Duration = Duration::from_secs(3);

pub fn db_lck_path(dbpath: &str) -> PathBuf {
    Path::new(dbpath).join("db.lck")
}

pub fn cleanup_on_signal(handle: &alpm::Alpm) {
    let raw = raw_alpm_handle(handle);
    let lock_path = db_lck_path(handle.dbpath());
    let signals = match signal_hook::iterator::Signals::new([
        signal_hook::consts::signal::SIGINT,
        signal_hook::consts::signal::SIGTERM,
        signal_hook::consts::signal::SIGHUP,
    ]) {
        Ok(signals) => signals,
        Err(e) => {
            eprintln!("[pakajo] failed to install signal handler for db.lck cleanup: {e}");
            return;
        }
    };
    std::thread::spawn(move || signal_loop(signals, lock_path, raw));
}

pub fn during_commit<T>(transaction: impl FnOnce() -> T) -> T {
    COMMIT_IN_FLIGHT.store(true, Ordering::Relaxed);
    let result = transaction();
    COMMIT_IN_FLIGHT.store(false, Ordering::Relaxed);
    result
}

pub fn finish_transaction(handle: &mut alpm::Alpm) {
    let _ = handle.trans_release();
    let sig = INTERRUPT_SIGNAL.load(Ordering::Relaxed);
    if sig != 0 {
        use std::io::Write as _;
        eprintln!("[pakajo] transaction stopped cleanly after signal {sig}");
        let _ = std::io::stdout().flush();
        std::process::exit(128 + sig);
    }
}

pub fn lock_retry<T>(
    mut op: impl FnMut() -> Result<T, alpm::Error>,
    on_wait: impl FnOnce(),
    delay: Duration,
) -> Result<T, alpm::Error> {
    let mut pending = Some(on_wait);
    loop {
        match op() {
            Err(alpm::Error::HandleLock) => {
                if let Some(notify) = pending.take() {
                    WAITING_FOR_LOCK.store(true, Ordering::Relaxed);
                    notify();
                }
                std::thread::sleep(delay);
            }
            settled => {
                WAITING_FOR_LOCK.store(false, Ordering::Relaxed);
                return settled;
            }
        }
    }
}

fn signal_loop(
    mut signals: signal_hook::iterator::Signals,
    lock_path: PathBuf,
    raw: RawAlpmHandle,
) {
    use std::io::Write as _;

    let RawAlpmHandle(raw) = raw;
    for sig in signals.forever() {
        if WAITING_FOR_LOCK.load(Ordering::Relaxed) {
            let _ = std::io::stdout().flush();
            std::process::exit(128 + sig);
        }
        let interrupting = COMMIT_IN_FLIGHT.load(Ordering::Relaxed)
            && unsafe { alpm_sys::alpm_trans_interrupt(raw) } == 0;
        if interrupting {
            eprintln!("[pakajo] interrupted by signal {sig}");
            INTERRUPT_SIGNAL.store(sig, Ordering::Relaxed);
            continue;
        }
        match std::fs::remove_file(&lock_path) {
            Ok(()) => eprintln!("[pakajo] interrupted by signal {sig}; removed db.lck"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[pakajo] interrupted by signal {sig}; no db.lck to remove")
            }
            Err(e) => {
                eprintln!("[pakajo] interrupted by signal {sig}; db.lck cleanup failed: {e}")
            }
        }
        let _ = std::io::stdout().flush();
        std::process::exit(128 + sig);
    }
}

// TODO: i really dislike this, but it's what pacman does and idk how to make it safe
struct RawAlpmHandle(*mut alpm_handle_t);

unsafe impl Send for RawAlpmHandle {}

fn raw_alpm_handle(handle: &alpm::Alpm) -> RawAlpmHandle {
    let raw = unsafe { std::ptr::read(std::ptr::from_ref(handle).cast()) };
    let dbpath = unsafe { std::ffi::CStr::from_ptr(alpm_sys::alpm_option_get_dbpath(raw)) };
    assert_eq!(
        dbpath.to_string_lossy(),
        handle.dbpath(),
        "handle dbpath mismatch"
    );
    RawAlpmHandle(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn lock_test_handle(suffix: &str) -> (alpm::Alpm, std::path::PathBuf) {
        let base =
            std::env::temp_dir().join(format!("pakajo_lock_{suffix}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let config = crate::pacman::config().unwrap();
        let handle = crate::pacman::init_alpm_at(
            &config,
            &root.to_string_lossy(),
            &db.to_string_lossy(),
            &[cache.to_string_lossy().into_owned()],
        )
        .unwrap();
        (handle, base)
    }

    fn lock_test_peer(base: &std::path::Path) -> alpm::Alpm {
        let config = crate::pacman::config().unwrap();
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        crate::pacman::init_alpm_at(
            &config,
            &root.to_string_lossy(),
            &db.to_string_lossy(),
            &[cache.to_string_lossy().into_owned()],
        )
        .unwrap()
    }

    #[test]
    fn db_lck_path_appends_db_lck() {
        assert_eq!(
            db_lck_path("/var/lib/pacman"),
            std::path::PathBuf::from("/var/lib/pacman/db.lck")
        );
    }

    #[test]
    fn db_lck_path_appends_db_lck_for_custom_dbpath() {
        assert_eq!(
            db_lck_path("/tmp/custom-pacman-db"),
            std::path::PathBuf::from("/tmp/custom-pacman-db/db.lck")
        );
    }

    #[test]
    fn lock_retry_waits_for_foreign_lock_release() {
        let (mut holder, base) = lock_test_handle("wait_succeed");
        holder.trans_init(alpm::TransFlag::NONE).unwrap();
        let lock_path = db_lck_path(holder.dbpath());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = std::fs::remove_file(lock_path);
        });
        let mut waiter = lock_test_peer(&base);
        let result = lock_retry(
            || waiter.trans_init(alpm::TransFlag::NONE),
            || {},
            Duration::from_millis(50),
        );
        assert!(result.is_ok());
        let _ = waiter.trans_release();
        let _ = holder.trans_release();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lock_retry_notifies_exactly_once() {
        let (mut holder, base) = lock_test_handle("wait_once");
        holder.trans_init(alpm::TransFlag::NONE).unwrap();
        let lock_path = db_lck_path(holder.dbpath());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(350));
            let _ = std::fs::remove_file(lock_path);
        });
        let mut waiter = lock_test_peer(&base);
        let calls = Cell::new(0);
        let result = lock_retry(
            || waiter.trans_init(alpm::TransFlag::NONE),
            || {
                calls.set(calls.get() + 1);
            },
            Duration::from_millis(50),
        );
        assert!(result.is_ok());
        assert_eq!(calls.get(), 1);
        let _ = waiter.trans_release();
        let _ = holder.trans_release();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lock_retry_propagates_non_lock_errors_immediately() {
        let attempts = Cell::new(0);
        let result: Result<(), alpm::Error> = lock_retry(
            || {
                attempts.set(attempts.get() + 1);
                Err(alpm::Error::Memory)
            },
            || {
                attempts.set(1000);
            },
            Duration::from_secs(10),
        );
        assert!(matches!(result, Err(alpm::Error::Memory)));
        assert_eq!(attempts.get(), 1);
    }
}
