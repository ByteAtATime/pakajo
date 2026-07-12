use std::cell::RefCell;
use std::io::Write as _;
use std::rc::Rc;

use anyhow::{Context, anyhow};

use alpm::DownloadResult as AlpmDownloadResult;
use alpm::LogLevel as AlpmLogLevel;
use alpm::Progress as AlpmProgress;

use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};

pub fn run_install<S: InstallSink + 'static, F: Fn() -> bool>(
    name: &str,
    sink: S,
    confirm: F,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::init_alpm(&config)?;
    install_into(&mut handle, name, sink, confirm)
}

fn install_into<S: InstallSink + 'static, F: Fn() -> bool>(
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

fn run_transaction<S: InstallSink, F: Fn() -> bool>(
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

pub struct ConsoleSink;

impl ConsoleSink {
    pub fn new() -> Self {
        Self
    }
}

impl InstallSink for ConsoleSink {
    fn event(&mut self, event: InstallEvent) {
        print_event(&event);
    }
}

fn print_event(event: &InstallEvent) {
    match event {
        InstallEvent::ResolvingDependencies => println!(":: resolving dependencies..."),
        InstallEvent::CheckingConflicts => println!(":: checking for conflicts..."),
        InstallEvent::CheckingFileConflicts => println!(":: checking for file conflicts..."),
        InstallEvent::CheckingIntegrity => println!(":: checking package integrity..."),
        InstallEvent::CheckingDiskSpace => println!(":: checking available disk space..."),
        InstallEvent::LoadingPackages => println!(":: loading package files..."),
        InstallEvent::KeyringStart => println!(":: checking keyring..."),
        InstallEvent::RetrievingPackages { num, total_bytes } => {
            println!(
                ":: retrieving {num} packages ({})",
                format_bytes(*total_bytes)
            );
        }
        InstallEvent::PackageOperation {
            operation,
            package,
            new_version,
            old_version,
        } => {
            println!(
                "{}",
                format_package_operation(*operation, package, new_version, old_version)
            );
        }
        InstallEvent::DownloadInit { filename, optional } => {
            if *optional {
                println!("  {filename} (optional)");
            }
        }
        InstallEvent::DownloadProgress {
            filename,
            downloaded,
            total,
        } => {
            print!(
                "\r  {filename}: {}/{}",
                format_bytes(*downloaded),
                format_bytes(*total)
            );
            let _ = std::io::stdout().flush();
        }
        InstallEvent::DownloadRetry { filename, resume } => {
            let kind = if *resume { "resumable" } else { "full" };
            println!("  {filename}: retrying ({kind})");
        }
        InstallEvent::DownloadCompleted {
            filename,
            total,
            result,
        } => {
            let status = match result {
                DownloadResult::Success => "done",
                DownloadResult::UpToDate => "up to date",
                DownloadResult::Failed => "failed",
            };
            println!("\r  {filename}: {} [{status}]", format_bytes(*total));
        }
        InstallEvent::Progress {
            phase,
            package,
            percent,
            current,
            total,
        } => {
            print_progress(*phase, package, *percent, *current, *total);
        }
        InstallEvent::HookRun {
            position,
            total,
            name,
            desc,
        } => {
            let label = desc.as_deref().unwrap_or(name);
            println!(":: running hook ({position}/{total}): {label}");
        }
        InstallEvent::ScriptletInfo { line } => println!("{line}"),
        InstallEvent::Log { level, message } => match level {
            LogLevel::Error => eprint!("error: {message}"),
            LogLevel::Warning => eprint!("warning: {message}"),
            LogLevel::Debug => {}
        },
        InstallEvent::TransactionDone => {}
        InstallEvent::TransactionSummary(s) => print_summary(s),
    }
}

fn print_summary(summary: &TransactionSummary) {
    if summary.packages.is_empty() {
        println!(" nothing to do");
        return;
    }

    let count = summary.packages.len();
    let rows: Vec<(String, String, String, String)> = summary
        .packages
        .iter()
        .map(|p| {
            (
                formatted_name(p),
                version_label(p),
                format_bytes(p.installed_size),
                format_bytes(p.download_size),
            )
        })
        .collect();

    let name_width = rows
        .iter()
        .map(|(name, _, _, _)| name.len())
        .max()
        .unwrap_or(0)
        .max(format!("Package ({count})").len());
    let version_width = rows
        .iter()
        .map(|(_, version, _, _)| version.len())
        .max()
        .unwrap_or(0)
        .max("Version".len());
    let installed_width = rows
        .iter()
        .map(|(_, _, installed, _)| installed.len())
        .max()
        .unwrap_or(0)
        .max("Installed Size".len());
    let download_width = rows
        .iter()
        .map(|(_, _, _, download)| download.len())
        .max()
        .unwrap_or(0)
        .max("Download Size".len());

    println!();
    println!(
        " {:<nw$}  {:<vw$}  {:>iw$}  {:>dw$}",
        format!("Package ({count})"),
        "Version",
        "Installed Size",
        "Download Size",
        nw = name_width,
        vw = version_width,
        iw = installed_width,
        dw = download_width,
    );
    println!();
    for (name, version, installed, download) in &rows {
        println!(
            " {:<nw$}  {:<vw$}  {:>iw$}  {:>dw$}",
            name,
            version,
            installed,
            download,
            nw = name_width,
            vw = version_width,
            iw = installed_width,
            dw = download_width,
        );
    }
    println!();
    println!(
        "Total Download Size:   {}",
        format_bytes(summary.total_download_size)
    );
    println!(
        "Total Installed Size:  {}",
        format_bytes(summary.total_installed_size)
    );
    println!();
}

fn formatted_name(pkg: &SummaryPackage) -> String {
    match &pkg.repository {
        Some(repo) => format!("{repo}/{}", pkg.name),
        None => pkg.name.clone(),
    }
}

fn version_label(pkg: &SummaryPackage) -> String {
    match &pkg.old_version {
        Some(old) => format!("{old} -> {}", pkg.new_version),
        None => pkg.new_version.clone(),
    }
}

pub fn confirm_install() -> bool {
    use std::io::Write as _;
    print!("\n:: Proceed with installation? [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

fn format_package_operation(
    operation: PackageOp,
    package: &str,
    new_version: &Option<String>,
    old_version: &Option<String>,
) -> String {
    let new = new_version.as_deref().unwrap_or("?");
    let old = old_version.as_deref().unwrap_or("?");
    match operation {
        PackageOp::Install => format!("installing {package} ({new})"),
        PackageOp::Upgrade => format!("upgrading {package} ({old} -> {new})"),
        PackageOp::Reinstall => format!("reinstalling {package} ({new})"),
        PackageOp::Downgrade => format!("downgrading {package} ({old} -> {new})"),
        PackageOp::Remove => format!("removing {package} ({old})"),
    }
}

fn print_progress(phase: ProgressPhase, package: &str, percent: i32, current: usize, total: usize) {
    let label = progress_phase_label(phase);
    let bar_width = 30;
    let filled = (percent as usize * bar_width / 100).min(bar_width);
    let bar: String = "#".repeat(filled);
    let spaces = "-".repeat(bar_width - filled);
    print!("\r{label} {package} ({current}/{total}) [{bar}{spaces}] {percent:>3}%");
    let _ = std::io::stdout().flush();
    if percent >= 100 {
        println!();
    }
}

fn progress_phase_label(phase: ProgressPhase) -> &'static str {
    match phase {
        ProgressPhase::Add => "installing",
        ProgressPhase::Upgrade => "upgrading",
        ProgressPhase::Downgrade => "downgrading",
        ProgressPhase::Reinstall => "reinstalling",
        ProgressPhase::Remove => "removing",
        ProgressPhase::Conflicts => "checking conflicts",
        ProgressPhase::Diskspace => "checking disk space",
        ProgressPhase::Integrity => "checking integrity",
        ProgressPhase::Load => "loading",
        ProgressPhase::Keyring => "checking keyring",
    }
}

fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} {}", UNITS[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
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
