use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use alpm_sys::alpm_handle_t;

static COMMIT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static INTERRUPT_SIGNAL: AtomicI32 = AtomicI32::new(0);

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

fn signal_loop(
    mut signals: signal_hook::iterator::Signals,
    lock_path: PathBuf,
    raw: RawAlpmHandle,
) {
    use std::io::Write as _;

    let RawAlpmHandle(raw) = raw;
    for sig in signals.forever() {
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
}
