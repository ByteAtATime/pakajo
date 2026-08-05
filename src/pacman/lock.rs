use std::path::{Path, PathBuf};

pub fn db_lck_path(dbpath: &str) -> PathBuf {
    Path::new(dbpath).join("db.lck")
}

pub fn install_lock_cleanup_on_signal(handle: &alpm::Alpm) {
    let lock_path = db_lck_path(handle.dbpath());

    #[cfg(not(test))]
    {
        install_lock_cleanup_on_signal_inner(lock_path);
    }

    #[cfg(test)]
    {
        let _ = handle;
        let _ = lock_path;
    }
}

#[cfg(not(test))]
#[allow(clippy::never_loop)]
fn install_lock_cleanup_on_signal_inner(lock_path: PathBuf) {
    use std::io::Write;

    let mut signals = match signal_hook::iterator::Signals::new([
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

    std::thread::spawn(move || {
        for sig in signals.forever() {
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
    });
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
