use std::io::Write as _;

use super::summary::{print_summary, render_summary};
use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase,
};
use crate::{color, utils::format_bytes};

pub(crate) struct ConsoleSink {
    last_progress: Option<(ProgressPhase, String, i32)>,
    hooks_header_done: bool,
    color: bool,
    stderr_color: bool,
}

impl ConsoleSink {
    pub(crate) fn new() -> Self {
        Self {
            last_progress: None,
            hooks_header_done: false,
            color: color::stdout_color(),
            stderr_color: color::stderr_color(),
        }
    }

    fn print_event(&mut self, event: &InstallEvent) {
        match event {
            InstallEvent::ResolvingDependencies => println!("resolving dependencies..."),
            InstallEvent::CheckingConflicts => println!("looking for conflicting packages..."),
            InstallEvent::CheckingFileConflicts => {},
            InstallEvent::CheckingIntegrity => {},
            InstallEvent::CheckingDiskSpace => {},
            InstallEvent::LoadingPackages => println!("loading packages..."),
            InstallEvent::KeyringStart => {},
            InstallEvent::RetrievingPackages { .. } => {
                println!("{}", color::colon(self.color, "Retrieving packages..."));
            }
            InstallEvent::ProcessingChanges => {
                println!("{}", color::colon(self.color, "Processing package changes..."));
            }
            InstallEvent::PackageOperation { .. } => {},
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
            } => match result {
                DownloadResult::UpToDate => {
                    println!(" {} is up to date", clean_pkg_filename(filename));
                }
                DownloadResult::Success => {
                    println!("\r  {filename}: {} [done]", format_bytes(*total));
                }
                DownloadResult::Failed => {
                    println!("\r  {filename}: {} [failed]", format_bytes(*total));
                }
            },
            InstallEvent::Progress {
                phase,
                package,
                percent,
                current,
                total,
            } => {
                let key = (*phase, package.clone(), *percent);
                if self.last_progress.as_ref() == Some(&key) {
                    return;
                }
                self.last_progress = Some(key);
                print_progress(*phase, package, *percent, *current, *total);
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
                if !self.hooks_header_done {
                    self.hooks_header_done = true;
                    println!("{}", color::colon(self.color, "Running post-transaction hooks..."));
                }
                let label = desc.as_deref().unwrap_or(name);
                println!("({position}/{total}) {label}");
            }
            InstallEvent::ScriptletInfo { line } => {
                if line.ends_with('\n') {
                    print!("{line}");
                } else {
                    println!("{line}");
                }
            }
            InstallEvent::Log { level, message } => match level {
                LogLevel::Error => eprint!("{} {message}", color::paint(self.stderr_color, color::RED, "error:")),
                LogLevel::Warning => eprint!(
                    "{} {message}",
                    color::paint(self.stderr_color, color::YELLOW, "warning:")
                ),
                LogLevel::Debug => {}
            },
            InstallEvent::TransactionDone => {}
            InstallEvent::TransactionSummary(s) => print_summary(s),
            InstallEvent::ResolvingAurDependencies { target } => {
                println!(
                    "{}",
                    color::colon(self.color, &format!("resolving dependencies for {}...", target))
                );
            }
            InstallEvent::AurDepResolved { .. } => {}
            InstallEvent::ResolutionComplete { .. } => {}
            InstallEvent::CloningRepo { .. } => {}
            InstallEvent::BuildStarted { .. } => {}
            InstallEvent::BuildOutput { line, .. } => {
                if self.color {
                    println!("{line}");
                } else {
                    println!("{}", color::ansi_strip(line));
                }
            }
            InstallEvent::BuildCompleted { .. } => {}
            InstallEvent::LayerBoundary { .. } => {}
            InstallEvent::SysupgradeAurCandidates { candidates } => {
                if candidates.is_empty() {
                    return;
                }
                println!(
                    "{}",
                    color::colon(
                        self.color,
                        &format!("{} AUR package(s) to upgrade:", candidates.len())
                    )
                );
                let name_width = candidates.iter().map(|c| c.name.len()).max().unwrap_or(0);
                let ver_width = candidates
                    .iter()
                    .map(|c| c.local_version.len().max(c.remote_version.len()))
                    .max()
                    .unwrap_or(0);
                for c in candidates {
                    println!(
                        "  {:<nw$}  {:<vw$} -> {:<vw$}",
                        c.name,
                        c.local_version,
                        c.remote_version,
                        nw = name_width,
                        vw = ver_width,
                    );
                }
            }
        }
    }
}

impl InstallSink for ConsoleSink {
    fn event(&mut self, event: InstallEvent) {
        self.print_event(&event);
    }
}

pub(super) struct JsonSink;

impl JsonSink {
    pub(super) fn new() -> Self {
        JsonSink
    }
}

impl InstallSink for JsonSink {
    fn event(&mut self, event: InstallEvent) {
        if let Ok(line) = serde_json::to_string(&event) {
            println!("{line}");
        }
    }
}

pub(super) struct EscalatedSink;

impl EscalatedSink {
    pub(super) fn new() -> Self {
        EscalatedSink
    }
}

impl InstallSink for EscalatedSink {
    fn event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::TransactionSummary(s) => {
                if s.packages.is_empty() {
                    eprintln!(" nothing to do");
                } else {
                    eprint!("{}", render_summary(&s, color::stderr_color()));
                }
                return;
            }
            other => {
                if let Ok(line) = serde_json::to_string(&other) {
                    println!("{line}");
                }
            }
        }
    }
}

fn clean_pkg_filename(name: &str) -> &str {
    let stripped = name.strip_suffix(".sig").unwrap_or(name);
    stripped.split(".pkg").next().unwrap_or(stripped)
}

fn print_progress(phase: ProgressPhase, package: &str, percent: i32, current: usize, total: usize) {
    let label = progress_phase_label(phase);
    let subject = if package.is_empty() {
        label.to_string()
    } else {
        format!("{label} {package}")
    };
    let bar_width = 30;
    let filled = (percent as usize * bar_width / 100).min(bar_width);
    let bar: String = "#".repeat(filled);
    let spaces = "-".repeat(bar_width - filled);
    print!("\r({current}/{total}) {subject} [{bar}{spaces}] {percent:>3}%");
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
        ProgressPhase::Conflicts => "checking for file conflicts",
        ProgressPhase::Diskspace => "checking available disk space",
        ProgressPhase::Integrity => "checking package integrity",
        ProgressPhase::Load => "loading package files",
        ProgressPhase::Keyring => "checking keys in keyring",
    }
}
