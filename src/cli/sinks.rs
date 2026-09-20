use std::collections::HashMap;
use std::io::Write as _;
use std::time::Instant;

use super::summary::{print_summary, render_summary};
use crate::color::{HIDE_CURSOR, SHOW_CURSOR};
use crate::events::{DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase};
use crate::{
    color,
    utils::{format_bytes, format_eta, format_rate, humanize_size},
};

struct DownloadStat {
    sync_xfered: i64,
    sync_time: Instant,
    rate: f64,
    eta: u64,
}

impl DownloadStat {
    fn observe(&mut self, now: Instant, downloaded: i64, total: i64) {
        let timediff = now.duration_since(self.sync_time).as_millis() as i64;
        if timediff >= 200 {
            let chunk = downloaded - self.sync_xfered;
            if chunk > 0 {
                let chunk_rate = chunk as f64 * 1000.0 / timediff as f64;
                self.rate = (chunk_rate + 2.0 * self.rate) / 3.0;
                if self.rate > 0.0 && total > self.sync_xfered {
                    self.eta = ((total - self.sync_xfered) as f64 / self.rate) as u64;
                }
            }
            self.sync_xfered = downloaded;
            self.sync_time = now;
        }
    }
}

pub struct ConsoleSink {
    last_progress: Option<(ProgressPhase, String, i32)>,
    hooks_header_done: bool,
    color: bool,
    stderr_color: bool,
    downloads: HashMap<String, DownloadStat>,
    cursor: Cursor,
}

impl ConsoleSink {
    pub fn new() -> Self {
        Self {
            last_progress: None,
            hooks_header_done: false,
            color: color::stdout_color(),
            stderr_color: color::stderr_color(),
            downloads: HashMap::new(),
            cursor: Cursor::new(color::stdout_color()),
        }
    }

    fn show_cursor(&mut self) {
        let show = self.cursor.show();
        if !show.is_empty() {
            print!("{show}");
            let _ = std::io::stdout().flush();
        }
    }

    fn hide_cursor(&mut self) {
        let hide = self.cursor.hide();
        if !hide.is_empty() {
            print!("{hide}");
            let _ = std::io::stdout().flush();
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
            InstallEvent::WaitingForDatabaseLock => {
                println!(
                    "{}",
                    color::colon(self.color, "Pacman is currently in use, please wait...")
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
                self.download_progress(filename, *downloaded, *total);
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
                self.downloads.remove(filename);
                match result {
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
                }
            }
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
                self.hide_cursor();
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
                println!(
                    "{}",
                    hook_run_line(*position, *total, name, desc.as_deref())
                );
            }
            InstallEvent::ScriptletInfo { line } => {
                if line.ends_with('\n') {
                    print!("{line}");
                } else {
                    println!("{line}");
                }
            }
            InstallEvent::Log { level, message } => match level {
                LogLevel::Error => eprintln!(
                    "{} {}",
                    color::paint(self.stderr_color, color::RED, "error:"),
                    message.trim_end()
                ),
                LogLevel::Warning => eprintln!(
                    "{} {}",
                    color::paint(self.stderr_color, color::YELLOW, "warning:"),
                    message.trim_end()
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
            InstallEvent::PkgbuildReviewStarted { .. }
            | InstallEvent::PkgbuildReviewAccepted { .. } => {}
            InstallEvent::PkgbuildAllUpToDate { packages } => {
                if packages.len() == 1 {
                    println!(
                        "{}",
                        color::colon(
                            self.color,
                            &format!("{}: already reviewed, no changes", packages[0])
                        )
                    );
                } else {
                    println!(
                        "{}",
                        color::colon(
                            self.color,
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

    fn download_progress(&mut self, filename: &str, downloaded: i64, total: i64) {
        self.hide_cursor();
        let now = Instant::now();
        let stat = self
            .downloads
            .entry(filename.to_string())
            .or_insert_with(|| DownloadStat {
                sync_xfered: downloaded,
                sync_time: now,
                rate: 0.0,
                eta: 0,
            });
        stat.observe(now, downloaded, total);
        let percent = if total > 0 {
            ((downloaded * 100 / total).min(100)) as i32
        } else {
            100
        };
        let rate = stat.rate;
        let eta = stat.eta;
        let cols = crate::utils::terminal_cols();
        let infolen = (cols * 6 / 10).max(50);
        let filenamelen = infolen.saturating_sub(30);
        let cell_width = cols.saturating_sub(infolen).saturating_sub(8);
        let fitted_name = fit_subject(clean_pkg_filename(filename), filenamelen);
        let (xval, xunit) = humanize_size(downloaded);
        let (rval, runit) = humanize_size(rate as i64);
        let rate_str = format_rate(rval);
        let eta_str = format_eta(eta);
        let bar = super::chomp::render(percent, cell_width, self.color);
        let clear = if self.color { "\x1b[K" } else { "" };
        print!(
            "\r {fitted_name} {xval:>6.1} {xunit:>3}  {rate_str} {runit:>3}/s {eta_str} {bar} {percent:>3}%{clear}"
        );
        let _ = std::io::stdout().flush();
    }
}

impl Default for ConsoleSink {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallSink for ConsoleSink {
    fn event(&mut self, event: InstallEvent) {
        self.print_event(&event);
    }
}

impl Drop for ConsoleSink {
    fn drop(&mut self) {
        self.show_cursor();
    }
}

pub struct JsonSink;

impl JsonSink {
    pub fn new() -> Self {
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

pub struct EscalatedSink {
    hooks_header_done: bool,
}

impl EscalatedSink {
    pub fn new() -> Self {
        EscalatedSink {
            hooks_header_done: false,
        }
    }
}

impl InstallSink for EscalatedSink {
    fn event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
                if !self.hooks_header_done {
                    self.hooks_header_done = true;
                    eprintln!(
                        "{}",
                        color::colon(color::stderr_color(), "Running post-transaction hooks...")
                    );
                }
                eprintln!("{}", hook_run_line(position, total, &name, desc.as_deref()));
            }
            InstallEvent::TransactionSummary(s) => {
                if s.packages.is_empty() {
                    eprintln!(" nothing to do");
                } else {
                    eprint!("{}", render_summary(&s, color::stderr_color()));
                }
            }
            InstallEvent::CheckingDependencies => {
                eprintln!("checking dependencies...");
            }
            InstallEvent::WaitingForDatabaseLock => {
                eprintln!(
                    "{}",
                    color::colon(
                        color::stderr_color(),
                        "Pacman is currently in use, please wait..."
                    )
                );
            }
            InstallEvent::Log {
                level: LogLevel::Warning,
                message,
            } => {
                eprintln!(
                    "{} {}",
                    color::paint(color::stderr_color(), color::YELLOW, "warning:"),
                    message.trim_end()
                );
            }
            InstallEvent::Log {
                level: LogLevel::Error,
                message,
            } => {
                eprintln!(
                    "{} {}",
                    color::paint(color::stderr_color(), color::RED, "error:"),
                    message.trim_end()
                );
            }
            other => {
                if let Ok(line) = serde_json::to_string(&other) {
                    println!("{line}");
                }
            }
        }
    }
}

fn hook_run_line(position: usize, total: usize, name: &str, desc: Option<&str>) -> String {
    let label = desc.unwrap_or(name);
    format!("({position}/{total}) {label}")
}

struct Cursor {
    hidden: bool,
    color: bool,
}

impl Cursor {
    fn new(color: bool) -> Self {
        Cursor {
            hidden: false,
            color,
        }
    }

    fn hide(&mut self) -> &'static str {
        if !self.color || self.hidden {
            ""
        } else {
            self.hidden = true;
            HIDE_CURSOR
        }
    }

    fn show(&mut self) -> &'static str {
        if self.hidden {
            self.hidden = false;
            SHOW_CURSOR
        } else {
            ""
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
    use std::time::Duration;

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

    #[test]
    fn observe_is_noop_within_200ms() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            sync_xfered: 1000,
            sync_time: base,
            rate: 5.0,
            eta: 42,
        };
        let before_rate = stat.rate;
        let before_eta = stat.eta;
        let before_sync = stat.sync_xfered;
        stat.observe(base + Duration::from_millis(100), 2000, 10000);
        assert_eq!(stat.rate, before_rate);
        assert_eq!(stat.eta, before_eta);
        assert_eq!(stat.sync_xfered, before_sync);
        assert_eq!(stat.sync_time, base);
    }

    #[test]
    fn observe_updates_rate_and_eta_on_first_sample() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            sync_xfered: 0,
            sync_time: base,
            rate: 0.0,
            eta: 0,
        };
        stat.observe(base + Duration::from_millis(300), 1000, 10000);
        let chunk_rate = 1000.0 * 1000.0 / 300.0;
        let expected_rate = (chunk_rate + 2.0 * 0.0) / 3.0;
        assert_eq!(stat.rate, expected_rate);
        assert_eq!(stat.eta, (10000.0 / expected_rate) as u64);
        assert_eq!(stat.sync_xfered, 1000);
        assert_eq!(stat.sync_time, base + Duration::from_millis(300));
    }

    #[test]
    fn observe_ema_accumulates_across_samples() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            sync_xfered: 0,
            sync_time: base,
            rate: 0.0,
            eta: 0,
        };
        stat.observe(base + Duration::from_millis(300), 1000, 10000);
        let chunk_rate0 = 1000.0 * 1000.0 / 300.0;
        let expected0 = (chunk_rate0 + 2.0 * 0.0) / 3.0;
        assert_eq!(stat.rate, expected0);
        assert_eq!(stat.eta, (10000.0 / expected0) as u64);
        assert_eq!(stat.sync_xfered, 1000);
        stat.observe(base + Duration::from_millis(600), 3000, 10000);
        let chunk_rate1 = 2000.0 * 1000.0 / 300.0;
        let expected1 = (chunk_rate1 + 2.0 * expected0) / 3.0;
        assert_eq!(stat.rate, expected1);
        assert_eq!(stat.eta, ((10000 - 1000) as f64 / expected1) as u64);
        assert_eq!(stat.sync_xfered, 3000);
        assert_eq!(stat.sync_time, base + Duration::from_millis(600));
    }
}
