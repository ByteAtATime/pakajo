use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context, anyhow};

use alpm::DownloadResult as AlpmDownloadResult;
use alpm::LogLevel as AlpmLogLevel;
use alpm::Progress as AlpmProgress;

use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};

pub fn run_install<S: InstallSink + 'static, F: FnOnce() -> bool>(
    name: &str,
    sink: S,
    confirm: F,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::init_alpm(&config)?;
    install_into(&mut handle, name, sink, confirm)
}

fn install_into<S: InstallSink + 'static, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    name: &str,
    sink: S,
    confirm: F,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    register_callbacks(handle, sink.clone());
    let result = run_transaction(handle, name, &sink, confirm);
    let _ = handle.trans_release();
    result
}

fn register_callbacks<S: InstallSink + 'static>(handle: &alpm::Alpm, sink: Rc<RefCell<S>>) {
    handle.set_event_cb(sink.clone(), |any_event, data| {
        if let Some(event) = convert_event(any_event) {
            data.borrow_mut().event(event);
        }
    });

    handle.set_dl_cb(sink.clone(), |filename, any_ev, data| {
        if let Some(event) = convert_download(filename, any_ev) {
            data.borrow_mut().event(event);
        }
    });

    handle.set_progress_cb(
        sink.clone(),
        |phase, pkgname, percent, howmany, current, data| {
            let event = InstallEvent::Progress {
                phase: convert_progress_phase(phase),
                package: pkgname.to_string(),
                percent,
                current,
                total: howmany,
            };
            data.borrow_mut().event(event);
        },
    );

    handle.set_log_cb(sink, |level, message, data| {
        if let Some(mapped) = convert_log_level(level) {
            data.borrow_mut().event(InstallEvent::Log {
                level: mapped,
                message: message.to_string(),
            });
        }
    });
}

fn run_transaction<S: InstallSink, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    name: &str,
    sink: &Rc<RefCell<S>>,
    confirm: F,
) -> anyhow::Result<()> {
    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;
    let pkg = crate::find_pkg(handle, name)
        .ok_or_else(|| anyhow!("package '{name}' not found in any repository"))?;
    handle
        .trans_add_pkg(pkg)
        .map_err(alpm::Error::from)
        .context("failed to queue package for installation")?;
    handle
        .trans_prepare()
        .map_err(alpm::Error::from)
        .context("failed to prepare transaction")?;

    let summary = build_summary(handle);
    sink.borrow_mut()
        .event(InstallEvent::TransactionSummary(summary));

    if !confirm() {
        return Ok(());
    }

    handle
        .trans_commit()
        .context("failed to commit transaction")?;
    Ok(())
}

fn build_summary(handle: &alpm::Alpm) -> TransactionSummary {
    let mut packages = Vec::new();
    let mut total_download_size = 0;
    let mut total_installed_size = 0;
    for pkg in handle.trans_add().iter() {
        let name = pkg.name().to_string();
        let old_version = handle
            .localdb()
            .pkg(name.as_str())
            .ok()
            .map(|p| p.version().to_string());
        let download_size = pkg.download_size();
        let installed_size = pkg.isize();
        total_download_size += download_size;
        total_installed_size += installed_size;
        packages.push(SummaryPackage {
            repository: pkg.db().map(|d| d.name().to_string()),
            new_version: pkg.version().to_string(),
            name,
            old_version,
            download_size,
            installed_size,
        });
    }
    TransactionSummary {
        packages,
        total_download_size,
        total_installed_size,
    }
}

fn convert_event(any_event: alpm::AnyEvent) -> Option<InstallEvent> {
    // libalpm emits type-only events (e.g. TransactionStart) as a 4-byte-aligned
    // alpm_event_any_t, but AnyEvent::event() derefs it as an 8-byte-aligned
    // alpm_event_t, tripping Rust's debug alignment check (non-unwinding abort).
    // Release builds tolerate the unaligned read of `type_`, so this skip is
    // debug-only; the skipped events carry no payload.
    // TODO: i think this is a bug in alpm, but I'm not really sure
    #[cfg(debug_assertions)]
    {
        let event_ptr: *const () = unsafe { std::mem::transmute_copy(&any_event) };
        if !(event_ptr as usize).is_multiple_of(std::mem::align_of::<*const ()>()) {
            return None;
        }
    }
    match any_event.event() {
        alpm::Event::ResolveDepsStart => Some(InstallEvent::ResolvingDependencies),
        alpm::Event::InterConflictsStart => Some(InstallEvent::CheckingConflicts),
        alpm::Event::FileConflictsStart => Some(InstallEvent::CheckingFileConflicts),
        alpm::Event::IntegrityStart => Some(InstallEvent::CheckingIntegrity),
        alpm::Event::DiskSpaceStart => Some(InstallEvent::CheckingDiskSpace),
        alpm::Event::LoadStart => Some(InstallEvent::LoadingPackages),
        alpm::Event::KeyringStart => Some(InstallEvent::KeyringStart),
        alpm::Event::PkgRetrieveStart(e) => Some(InstallEvent::RetrievingPackages {
            num: e.num(),
            total_bytes: e.total_size(),
        }),
        alpm::Event::PackageOperationStart(e) => {
            let (operation, package, new_version, old_version) =
                convert_package_operation(e.operation());
            Some(InstallEvent::PackageOperation {
                operation,
                package,
                new_version,
                old_version,
            })
        }
        alpm::Event::ScriptletInfo(e) => Some(InstallEvent::ScriptletInfo {
            line: e.line().to_string(),
        }),
        alpm::Event::HookRunStart(e) => Some(InstallEvent::HookRun {
            position: e.position(),
            total: e.total(),
            name: e.name().to_string(),
            desc: e.desc().map(str::to_string),
        }),
        alpm::Event::TransactionDone => Some(InstallEvent::TransactionDone),
        _ => None,
    }
}

fn convert_package_operation(
    op: alpm::PackageOperation,
) -> (PackageOp, String, Option<String>, Option<String>) {
    let (operation, new, old): (PackageOp, Option<&alpm::Package>, Option<&alpm::Package>) =
        match op {
            alpm::PackageOperation::Install(p) => (PackageOp::Install, Some(p), None),
            alpm::PackageOperation::Upgrade(n, o) => (PackageOp::Upgrade, Some(n), Some(o)),
            alpm::PackageOperation::Reinstall(n, o) => (PackageOp::Reinstall, Some(n), Some(o)),
            alpm::PackageOperation::Downgrade(n, o) => (PackageOp::Downgrade, Some(n), Some(o)),
            alpm::PackageOperation::Remove(p) => (PackageOp::Remove, None, Some(p)),
        };
    let package = new
        .or(old)
        .map(|p| p.name().to_string())
        .unwrap_or_default();
    (
        operation,
        package,
        new.map(|p| p.version().to_string()),
        old.map(|p| p.version().to_string()),
    )
}

fn convert_download(filename: &str, any_ev: alpm::AnyDownloadEvent) -> Option<InstallEvent> {
    let filename = filename.to_string();
    Some(match any_ev.event() {
        alpm::DownloadEvent::Init(init) => InstallEvent::DownloadInit {
            filename,
            optional: init.optional,
        },
        alpm::DownloadEvent::Progress(p) => InstallEvent::DownloadProgress {
            filename,
            downloaded: p.downloaded,
            total: p.total,
        },
        alpm::DownloadEvent::Retry(r) => InstallEvent::DownloadRetry {
            filename,
            resume: r.resume,
        },
        alpm::DownloadEvent::Completed(c) => InstallEvent::DownloadCompleted {
            filename,
            total: c.total,
            result: convert_download_result(c.result),
        },
    })
}

fn convert_download_result(result: AlpmDownloadResult) -> DownloadResult {
    match result {
        AlpmDownloadResult::Success => DownloadResult::Success,
        AlpmDownloadResult::UpToDate => DownloadResult::UpToDate,
        AlpmDownloadResult::Failed => DownloadResult::Failed,
    }
}

fn convert_progress_phase(phase: AlpmProgress) -> ProgressPhase {
    match phase {
        AlpmProgress::AddStart => ProgressPhase::Add,
        AlpmProgress::UpgradeStart => ProgressPhase::Upgrade,
        AlpmProgress::DowngradeStart => ProgressPhase::Downgrade,
        AlpmProgress::ReinstallStart => ProgressPhase::Reinstall,
        AlpmProgress::RemoveStart => ProgressPhase::Remove,
        AlpmProgress::ConflictsStart => ProgressPhase::Conflicts,
        AlpmProgress::DiskspaceStart => ProgressPhase::Diskspace,
        AlpmProgress::IntegrityStart => ProgressPhase::Integrity,
        AlpmProgress::LoadStart => ProgressPhase::Load,
        AlpmProgress::KeyringStart => ProgressPhase::Keyring,
    }
}

fn convert_log_level(level: AlpmLogLevel) -> Option<LogLevel> {
    if level.contains(AlpmLogLevel::ERROR) {
        Some(LogLevel::Error)
    } else if level.contains(AlpmLogLevel::WARNING) {
        Some(LogLevel::Warning)
    } else if level.contains(AlpmLogLevel::DEBUG) {
        Some(LogLevel::Debug)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConsoleSink;
    use std::fs;

    #[test]
    #[ignore]
    fn test_install() {
        let base = std::env::temp_dir().join("pakajo_fake_root");
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&db).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let cache_str = cache.to_string_lossy().into_owned();
        let config = pacmanconf::Config::new().unwrap();
        let mut handle = crate::init_alpm_at(
            &config,
            &root.to_string_lossy(),
            &db.to_string_lossy(),
            &[cache_str],
        )
        .unwrap();
        handle.syncdbs_mut().update(false).unwrap();
        let result = install_into(&mut handle, "sl", ConsoleSink::new(), || true);
        let _ = handle.trans_release();
        result.expect("install should succeed");
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl should be installed in the local db"
        );
    }

    #[test]
    #[ignore]
    fn test_install_aborted() {
        let base = std::env::temp_dir().join("pakajo_fake_root_abort");
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&db).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let cache_str = cache.to_string_lossy().into_owned();
        let config = pacmanconf::Config::new().unwrap();
        let mut handle = crate::init_alpm_at(
            &config,
            &root.to_string_lossy(),
            &db.to_string_lossy(),
            &[cache_str],
        )
        .unwrap();
        handle.syncdbs_mut().update(false).unwrap();
        let result = install_into(&mut handle, "sl", ConsoleSink::new(), || false);
        let _ = handle.trans_release();
        result.expect("aborted install should not error");
        assert!(
            handle.localdb().pkg("sl").is_err(),
            "sl must NOT be installed after an aborted confirm"
        );
    }
}
