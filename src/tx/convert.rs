use alpm::DownloadResult as AlpmDownloadResult;
use alpm::LogLevel as AlpmLogLevel;
use alpm::Progress as AlpmProgress;

use crate::events::{
    DownloadResult, InstallEvent, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};

pub fn build_summary(handle: &alpm::Alpm) -> TransactionSummary {
    let mut packages = Vec::new();
    let mut total_download_size = 0;
    let mut total_installed_size = 0;
    let mut total_removed_size = 0;
    for pkg in handle.trans_add().iter() {
        let name = pkg.name().to_string();
        let (old_version, old_installed_size) = handle
            .localdb()
            .pkg(name.as_str())
            .ok()
            .map(|p| (Some(p.version().to_string()), p.isize()))
            .unwrap_or((None, 0));
        let download_size = pkg.download_size();
        let installed_size = pkg.isize();
        total_download_size += download_size;
        total_installed_size += installed_size;
        if old_installed_size > 0 {
            total_removed_size += old_installed_size;
        }
        packages.push(SummaryPackage {
            repository: pkg.db().map(|d| d.name().to_string()),
            new_version: pkg.version().to_string(),
            name,
            old_version,
            download_size,
            installed_size,
            old_installed_size,
            is_removal: false,
        });
    }
    for pkg in handle.trans_remove().iter() {
        let name = pkg.name().to_string();
        let old_version = pkg.version().to_string();
        let installed_size = pkg.isize();
        total_removed_size += installed_size;
        packages.push(SummaryPackage {
            repository: None,
            new_version: String::new(),
            name,
            old_version: Some(old_version),
            download_size: 0,
            installed_size,
            old_installed_size: 0,
            is_removal: true,
        });
    }
    TransactionSummary {
        packages,
        total_download_size,
        total_installed_size,
        total_removed_size,
    }
}

pub(crate) fn convert_event(any_event: alpm::AnyEvent) -> Option<InstallEvent> {
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
        alpm::Event::CheckDepsStart => Some(InstallEvent::CheckingDependencies),
        alpm::Event::FileConflictsStart => Some(InstallEvent::CheckingFileConflicts),
        alpm::Event::IntegrityStart => Some(InstallEvent::CheckingIntegrity),
        alpm::Event::DiskSpaceStart => Some(InstallEvent::CheckingDiskSpace),
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
        alpm::Event::TransactionStart => Some(InstallEvent::ProcessingChanges),
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

pub(crate) fn convert_download(
    filename: &str,
    any_ev: alpm::AnyDownloadEvent,
) -> Option<InstallEvent> {
    if filename.ends_with(".sig") {
        return None;
    }
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

pub(crate) fn convert_progress_phase(phase: AlpmProgress) -> ProgressPhase {
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

pub(crate) fn convert_log_level(level: AlpmLogLevel) -> Option<LogLevel> {
    if level.contains(AlpmLogLevel::ERROR) {
        Some(LogLevel::Error)
    } else if level.contains(AlpmLogLevel::WARNING) {
        Some(LogLevel::Warning)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub enum PrepareFailure {
    Unsatisfied(Vec<UnsatisfiedDep>),
    Other(String),
}

#[derive(Debug, Clone)]
pub struct UnsatisfiedDep {
    pub depend: String,
    pub target: String,
}

pub(crate) fn extract_prepare_failure(err: alpm::PrepareError) -> PrepareFailure {
    match err.data() {
        Some(alpm::PrepareData::UnsatisfiedDeps(list)) => PrepareFailure::Unsatisfied(
            list.iter()
                .map(|d| UnsatisfiedDep {
                    depend: d.depend().name().to_string(),
                    target: d.target().to_string(),
                })
                .collect(),
        ),
        Some(other) => PrepareFailure::Other(format!("{other:?}")),
        None => PrepareFailure::Other(format!("{}", err.error())),
    }
}
