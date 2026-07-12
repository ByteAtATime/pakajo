use std::io::Write as _;

use anyhow::Context as _;

use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};
use crate::install;
use crate::utils::format_bytes;

pub(crate) fn install_subcommand(mut args: impl Iterator<Item = String>) -> ! {
    let name = match args.next() {
        Some(name) => name,
        None => {
            eprintln!("usage: pakajo install <package>");
            std::process::exit(2);
        }
    };

    if unsafe { libc::geteuid() } != 0 {
        let exe = match std::env::current_exe().context("failed to determine executable path") {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
        let status = match std::process::Command::new("sudo")
            .arg(&exe)
            .arg("install")
            .arg(&name)
            .status()
            .context("failed to run sudo")
        {
            Ok(status) => status,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
        std::process::exit(status.code().unwrap_or(1));
    }

    match install::run_install(&name, ConsoleSink::new(), confirm_install) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

pub(crate) struct ConsoleSink;

impl ConsoleSink {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl InstallSink for ConsoleSink {
    fn event(&mut self, event: InstallEvent) {
        print_event(&event);
    }
}

fn confirm_install() -> bool {
    print!("\n:: Proceed with installation? [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
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
