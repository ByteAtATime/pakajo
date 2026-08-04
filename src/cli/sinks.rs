use std::io::Write as _;

use super::summary::{print_summary, render_summary};
use crate::events::{DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase};
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
            InstallEvent::CheckingDependencies => println!("checking dependencies..."),
            InstallEvent::CheckingFileConflicts => {}
            InstallEvent::CheckingIntegrity => {}
            InstallEvent::CheckingDiskSpace => {}
            InstallEvent::LoadingPackages => println!("loading packages..."),
            InstallEvent::KeyringStart => {}
            InstallEvent::RetrievingPackages { .. } => {
                println!("{}", color::colon(self.color, "Retrieving packages..."));
            }
            InstallEvent::ProcessingChanges => {
                println!(
                    "{}",
                    color::colon(self.color, "Processing package changes...")
                );
            }
            InstallEvent::PackageOperation { .. } => {}
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
                let percent = if *total > 0 {
                    ((*downloaded * 100 / *total).min(100)) as i32
                } else {
                    100
                };
                let cols = crate::utils::terminal_cols();
                let infolen = (cols * 6 / 10).max(50);
                let filename_width = infolen.saturating_sub(2 + 20);
                let cell_width = cols.saturating_sub(infolen).saturating_sub(8);
                let fitted_name = fit_subject(filename, filename_width);
                let bytes = format!("{}/{}", format_bytes(*downloaded), format_bytes(*total));
                let bar = super::chomp::render(percent, cell_width, self.color);
                let clear = if self.color { "\x1b[K" } else { "" };
                print!("\r  {fitted_name}{bytes:<20} {bar} {percent:>3}%{clear}");
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
                    let clear = if self.color { "\x1b[K" } else { "" };
                    println!("\r  {filename}: {} [done]{clear}", format_bytes(*total));
                }
                DownloadResult::Failed => {
                    let clear = if self.color { "\x1b[K" } else { "" };
                    println!("\r  {filename}: {} [failed]{clear}", format_bytes(*total));
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
                print_progress(*phase, package, *percent, *current, *total, self.color);
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
                if !self.hooks_header_done {
                    self.hooks_header_done = true;
                    println!(
                        "{}",
                        color::colon(self.color, "Running post-transaction hooks...")
                    );
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
                LogLevel::Error => eprint!(
                    "{} {message}",
                    color::paint(self.stderr_color, color::RED, "error:")
                ),
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
                    color::colon(
                        self.color,
                        &format!("resolving dependencies for {}...", target)
                    )
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
            InstallEvent::PkgbuildReviewStarted { .. }
            | InstallEvent::PkgbuildReviewAccepted { .. } => {}
            InstallEvent::PkgbuildAllUpToDate { packages } => {
                let c = crate::color::stdout_color();
                if packages.len() == 1 {
                    println!(
                        "{}",
                        color::colon(c, &format!("{}: already reviewed, no changes", packages[0]))
                    );
                } else {
                    println!(
                        "{}",
                        color::colon(
                            c,
                            &format!("{} packages already reviewed, no changes", packages.len())
                        )
                    );
                }
            }
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
            InstallEvent::CheckingDependencies => {
                eprintln!("checking dependencies...");
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

fn print_progress(
    phase: ProgressPhase,
    package: &str,
    percent: i32,
    current: usize,
    total: usize,
    colored: bool,
) {
    let label = progress_phase_label(phase);
    let subject = if package.is_empty() {
        label.to_string()
    } else {
        format!("{label} {package}")
    };
    let cols = crate::utils::terminal_cols();
    let infolen = (cols * 6 / 10).max(50);
    let digits = count_digits(total);
    let textlen = infolen.saturating_sub(3 + 2 * digits + 1);
    let cell_width = cols.saturating_sub(infolen).saturating_sub(8);
    let header = format!("({:>w$}/{:>w$})", current, total, w = digits);
    let fitted = fit_subject(&subject, textlen);
    let bar = super::chomp::render(percent, cell_width, colored);
    print!("\r{header} {fitted} {bar} {percent:>3}%");
    let _ = std::io::stdout().flush();
    if percent >= 100 {
        println!();
    }
}

fn count_digits(mut n: usize) -> usize {
    let mut digits = 1;
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits
}

fn fit_subject(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len > width {
        if width <= 3 {
            s.chars().take(width).collect()
        } else {
            let kept: String = s.chars().take(width - 3).collect();
            format!("{kept}...")
        }
    } else {
        format!("{s:width$}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_digits_handles_boundaries() {
        assert_eq!(count_digits(0), 1);
        assert_eq!(count_digits(9), 1);
        assert_eq!(count_digits(10), 2);
        assert_eq!(count_digits(100), 3);
    }

    #[test]
    fn fit_subject_pads_short_labels() {
        assert_eq!(fit_subject("hi", 5), "hi   ");
    }

    #[test]
    fn fit_subject_truncates_with_ellipsis() {
        assert_eq!(fit_subject("hello world", 6), "hel...");
    }

    #[test]
    fn fit_subject_keeps_exact_fit() {
        assert_eq!(fit_subject("hello", 5), "hello");
    }
}
