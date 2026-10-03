use alpm::DownloadResult as AlpmDownloadResult;
use alpm::LogLevel as AlpmLogLevel;
use alpm::Progress as AlpmProgress;
use serde::{Deserialize, Serialize};

use crate::events::{
    DownloadResult, InstallEvent, LogLevel, MergeOrigin, PackageOp, ProgressPhase, SummaryPackage,
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
        alpm::Event::HookStart(e) => Some(InstallEvent::HookStart {
            pre: e.when() == alpm::HookWhen::PreTransaction,
        }),
        alpm::Event::HookRunStart(e) => Some(InstallEvent::HookRun {
            position: e.position(),
            total: e.total(),
            name: e.name().to_string(),
            desc: e.desc().map(str::to_string),
        }),
        alpm::Event::TransactionDone => Some(InstallEvent::TransactionDone),
        alpm::Event::TransactionStart => Some(InstallEvent::ProcessingChanges),
        alpm::Event::ResolveDepsDone => Some(InstallEvent::ResolveDepsDone),
        alpm::Event::CheckDepsDone => Some(InstallEvent::CheckDepsDone),
        alpm::Event::InterConflictsDone => Some(InstallEvent::InterConflictsDone),
        alpm::Event::FileConflictsDone => Some(InstallEvent::FileConflictsDone),
        alpm::Event::IntegrityDone => Some(InstallEvent::IntegrityDone),
        alpm::Event::LoadStart => Some(InstallEvent::LoadingPackages),
        alpm::Event::LoadDone => Some(InstallEvent::LoadDone),
        alpm::Event::DiskSpaceDone => Some(InstallEvent::DiskSpaceDone),
        alpm::Event::KeyringDone => Some(InstallEvent::KeyringDone),
        alpm::Event::KeyDownloadStart => Some(InstallEvent::KeyDownloadStart),
        alpm::Event::KeyDownloadDone => Some(InstallEvent::KeyDownloadDone),
        alpm::Event::RetrieveStart => Some(InstallEvent::RetrieveStart),
        alpm::Event::RetrieveDone => Some(InstallEvent::RetrieveDone),
        alpm::Event::RetrieveFailed => Some(InstallEvent::RetrieveFailed),
        alpm::Event::PkgRetrieveDone(e) => Some(InstallEvent::PkgRetrieveDone {
            num: e.num(),
            total_bytes: e.total_size(),
        }),
        alpm::Event::PkgRetrieveFailed(e) => Some(InstallEvent::PkgRetrieveFailed {
            num: e.num(),
            total_bytes: e.total_size(),
        }),
        alpm::Event::PackageOperationDone(e) => {
            let (operation, package, _, _) = convert_package_operation(e.operation());
            Some(InstallEvent::PackageOperationEnd { operation, package })
        }
        alpm::Event::HookDone(e) => Some(InstallEvent::HookDone {
            pre: e.when() == alpm::HookWhen::PreTransaction,
        }),
        alpm::Event::HookRunDone(_) => Some(InstallEvent::HookRunDone),
        alpm::Event::OptDepRemoval(e) => Some(InstallEvent::OptDepRemoval {
            package: e.pkg().name().to_string(),
            optdep: e.optdep().to_string(),
        }),
        alpm::Event::DatabaseMissing(e) => Some(InstallEvent::DatabaseMissing {
            dbname: e.dbname().to_string(),
        }),
        alpm::Event::PacnewCreated(e) => Some(InstallEvent::PacnewCreated {
            from_noupgrade: e.from_noupgrade(),
            file: e.file().to_string(),
            origin: MergeOrigin::of(e.oldpkg(), e.newpkg()),
        }),
        alpm::Event::PacsaveCreated(e) => Some(InstallEvent::PacsaveCreated {
            file: e.file().to_string(),
            origin: MergeOrigin::of(e.oldpkg(), None),
        }),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsatisfiedDep {
    pub cause: Option<String>,
    pub depend: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct PrepareFailure {
    reason: String,
    unsatisfied: Vec<UnsatisfiedDep>,
}

impl PrepareFailure {
    pub fn new(reason: impl Into<String>, details: Vec<UnsatisfiedDep>) -> Self {
        Self {
            reason: reason.into(),
            unsatisfied: details,
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn unsatisfied(&self) -> &[UnsatisfiedDep] {
        &self.unsatisfied
    }

    pub fn details(&self) -> Vec<String> {
        self.unsatisfied.iter().map(render_unsatisfied).collect()
    }

    pub fn report(&self, colored: bool) -> String {
        let header = format!(
            "{} failed to prepare transaction ({})",
            crate::color::paint(colored, crate::color::RED, "error:"),
            self.reason
        );
        std::iter::once(header)
            .chain(
                self.unsatisfied
                    .iter()
                    .map(|d| crate::color::colon(colored, &render_unsatisfied(d))),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl std::fmt::Display for PrepareFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.report(false))
    }
}

impl std::error::Error for PrepareFailure {}

fn unsatisfied_dep(miss: &alpm::DepMissing) -> UnsatisfiedDep {
    UnsatisfiedDep {
        cause: miss.causing_pkg().map(str::to_string),
        depend: miss.depend().to_string(),
        target: miss.target().to_string(),
    }
}

fn render_unsatisfied(dep: &UnsatisfiedDep) -> String {
    match &dep.cause {
        Some(cause) => format!(
            "removing {cause} breaks dependency '{}' required by {}",
            dep.depend, dep.target
        ),
        None => format!(
            "unable to satisfy dependency '{}' required by {}",
            dep.depend, dep.target
        ),
    }
}

pub(crate) fn extract_prepare_failure(err: alpm::PrepareError) -> PrepareFailure {
    let reason = err.error().to_string();
    let unsatisfied = match err.data() {
        Some(alpm::PrepareData::UnsatisfiedDeps(list)) => {
            list.iter().map(unsatisfied_dep).collect()
        }
        _ => Vec::new(),
    };
    PrepareFailure::new(reason, unsatisfied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(cause: Option<&str>, depend: &str, target: &str) -> UnsatisfiedDep {
        UnsatisfiedDep {
            cause: cause.map(str::to_string),
            depend: depend.to_string(),
            target: target.to_string(),
        }
    }

    fn failure(reason: &str, details: Vec<UnsatisfiedDep>) -> PrepareFailure {
        PrepareFailure::new(reason.to_string(), details)
    }

    #[test]
    fn removal_break_reports_reason_then_detail() {
        let broke = failure(
            "could not satisfy dependencies",
            vec![dep(
                Some("shelly-bin"),
                "shelly-bin=3.1.6-1",
                "shelly-flatpak-backend-bin",
            )],
        );
        assert_eq!(
            broke.report(false),
            "error: failed to prepare transaction (could not satisfy dependencies)\n:: removing shelly-bin breaks dependency 'shelly-bin=3.1.6-1' required by shelly-flatpak-backend-bin"
        );
    }

    #[test]
    fn every_missing_dep_gets_its_own_detail_line() {
        let many = failure(
            "could not satisfy dependencies",
            vec![
                dep(Some("glibc"), "libfoo>=2", "sl"),
                dep(None, "libbar", "vlc"),
            ],
        );
        assert_eq!(
            many.report(false),
            "error: failed to prepare transaction (could not satisfy dependencies)\n:: removing glibc breaks dependency 'libfoo>=2' required by sl\n:: unable to satisfy dependency 'libbar' required by vlc"
        );
    }

    #[test]
    fn reason_only_failure_has_no_detail_lines() {
        let broken = failure("database not found", Vec::new());
        assert_eq!(
            broken.report(false),
            "error: failed to prepare transaction (database not found)"
        );
    }

    #[test]
    fn colored_report_paints_without_changing_text() {
        let broke = failure(
            "could not satisfy dependencies",
            vec![dep(Some("glibc"), "libfoo", "sl")],
        );
        let plain = broke.report(false);
        let painted = broke.report(true);
        assert_ne!(plain, painted);
        assert_eq!(crate::color::ansi_strip(&painted), plain);
    }

    #[test]
    fn shelly_fixture_carries_typed_fields() {
        let broke = failure(
            "could not satisfy dependencies",
            vec![dep(
                Some("shelly-bin"),
                "shelly-bin=3.1.6-1",
                "shelly-flatpak-backend-bin",
            )],
        );
        assert_eq!(broke.reason(), "could not satisfy dependencies");
        assert_eq!(
            broke.unsatisfied(),
            &[UnsatisfiedDep {
                cause: Some("shelly-bin".to_string()),
                depend: "shelly-bin=3.1.6-1".to_string(),
                target: "shelly-flatpak-backend-bin".to_string(),
            }]
        );
        assert_eq!(
            broke.details(),
            vec![
                "removing shelly-bin breaks dependency 'shelly-bin=3.1.6-1' required by shelly-flatpak-backend-bin"
                    .to_string()
            ]
        );
    }
}
