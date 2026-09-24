use std::collections::HashMap;
use std::io::Write as _;
use std::time::Instant;

use super::summary::{print_summary, render_summary};
use crate::color::{HIDE_CURSOR, SHOW_CURSOR};
use crate::events::{DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase};
use crate::{
    color,
    utils::{format_eta, format_rate},
};

const DOWNLOAD_SAMPLE_MS: u128 = 200;
const ETA_UNKNOWN: u64 = u32::MAX as u64;

struct DownloadStat {
    xfered: i64,
    total: i64,
    init_time: Instant,
    sync_xfered: i64,
    sync_time: Option<Instant>,
    rate: f64,
    eta: u64,
}

impl DownloadStat {
    fn fresh(now: Instant) -> Self {
        DownloadStat {
            xfered: 0,
            total: 0,
            init_time: now,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        }
    }

    fn reset(&mut self, now: Instant) {
        self.xfered = 0;
        self.total = 0;
        self.init_time = now;
        self.sync_xfered = 0;
        self.sync_time = None;
        self.rate = 0.0;
        self.eta = 0;
    }

    fn derive_eta(&mut self, total: i64) {
        if self.rate > 0.0 {
            let remaining = (total - self.sync_xfered) as f64 / self.rate;
            self.eta = if remaining < 0.0 || remaining > ETA_UNKNOWN as f64 {
                ETA_UNKNOWN
            } else {
                remaining as u64
            };
        } else {
            self.eta = ETA_UNKNOWN;
        }
    }

    fn observe(&mut self, now: Instant, downloaded: i64, total: i64) -> bool {
        if downloaded < 0 || total < 0 {
            return false;
        }
        self.xfered = downloaded;
        self.total = total;
        match self.sync_time {
            None => {
                self.sync_xfered = downloaded;
                self.sync_time = Some(now);
                if downloaded > 0 {
                    self.rate = f64::MIN_POSITIVE;
                } else {
                    self.rate = 0.0;
                }
                self.derive_eta(total);
                true
            }
            Some(previous) => {
                let elapsed = now.saturating_duration_since(previous).as_millis();
                if elapsed < DOWNLOAD_SAMPLE_MS {
                    return false;
                }
                let chunk = downloaded - self.sync_xfered;
                self.sync_xfered = downloaded;
                self.sync_time = Some(now);
                if chunk > 0 && elapsed > 0 {
                    let chunk_rate = chunk as f64 * 1000.0 / elapsed as f64;
                    self.rate = (chunk_rate + 2.0 * self.rate) / 3.0;
                }
                self.derive_eta(total);
                true
            }
        }
    }

    fn finish(&mut self, now: Instant, total: i64) {
        let total = total.max(0);
        self.xfered = total;
        self.total = total;
        self.sync_xfered = total;
        let elapsed = now
            .saturating_duration_since(self.init_time)
            .as_millis()
            .max(1);
        self.rate = total as f64 * 1000.0 / elapsed as f64;
        self.eta = ((elapsed + 500) / 1000).min(u128::from(u64::MAX)) as u64;
    }
}

struct DownloadBars {
    stats: HashMap<String, DownloadStat>,
}

impl DownloadBars {
    fn new() -> Self {
        DownloadBars {
            stats: HashMap::new(),
        }
    }

    fn init_file(&mut self, filename: &str, now: Instant) {
        self.stats
            .insert(filename.to_string(), DownloadStat::fresh(now));
    }

    fn progress_line(
        &mut self,
        filename: &str,
        downloaded: i64,
        total: i64,
        now: Instant,
        cols: usize,
    ) -> Option<String> {
        let stat = self
            .stats
            .entry(filename.to_string())
            .or_insert_with(|| DownloadStat::fresh(now));
        if !stat.observe(now, downloaded, total) {
            return None;
        }
        Some(draw_download_bar(
            clean_pkg_filename(filename),
            stat.sync_xfered,
            stat.total,
            stat.rate,
            stat.eta,
            cols,
        ))
    }

    fn retry_file(&mut self, filename: &str, now: Instant) {
        if let Some(stat) = self.stats.get_mut(filename) {
            stat.reset(now);
        }
    }

    fn up_to_date_line(&mut self, filename: &str, color: bool) -> String {
        self.stats.remove(filename);
        let clear = if color { "\x1b[K" } else { "" };
        format!(" {} is up to date{clear}", clean_pkg_filename(filename))
    }

    fn failed_line(&mut self, filename: &str, color: bool) -> String {
        self.stats.remove(filename);
        let clear = if color { "\x1b[K" } else { "" };
        format!(" {filename} failed to download{clear}")
    }

    fn success_line(&mut self, filename: &str, total: i64, now: Instant, cols: usize) -> String {
        let stat = self
            .stats
            .entry(filename.to_string())
            .or_insert_with(|| DownloadStat::fresh(now));
        stat.finish(now, total);
        let line = draw_download_bar(
            clean_pkg_filename(filename),
            stat.sync_xfered,
            stat.total,
            stat.rate,
            stat.eta,
            cols,
        );
        self.stats.remove(filename);
        line
    }
}

fn download_percent(xfered: i64, total: i64) -> i32 {
    if total <= 0 {
        return 100;
    }
    ((xfered * 100 / total).clamp(0, 100)) as i32
}

fn pacman_humanize(value: f64) -> (f64, &'static str) {
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let mut scaled = value;
    let mut unit = 0;
    while !(-2048.0..=2048.0).contains(&scaled) && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    (scaled, UNITS[unit])
}

fn fill_progress(percent: i32, proglen: i64) -> String {
    let percent = percent.clamp(0, 100);
    let hashlen = if proglen > 8 { proglen - 8 } else { 0 };
    let mut bar = String::new();
    if hashlen > 0 {
        let hash = i64::from(percent) * hashlen / 100;
        bar.push_str(" [");
        for position in 0..hashlen {
            if position < hash {
                bar.push('#');
            } else {
                bar.push('-');
            }
        }
        bar.push(']');
    }
    if proglen >= 5 {
        bar.push_str(&format!(" {percent:>3}%"));
    }
    bar
}

fn draw_download_bar(
    cleaned: &str,
    xfered: i64,
    total: i64,
    rate: f64,
    eta: u64,
    cols: usize,
) -> String {
    let percent = download_percent(xfered, total);
    let infolen = (cols * 6 / 10).max(50);
    let filenamelen = infolen.saturating_sub(30);
    let fitted = fit_subject(cleaned, filenamelen);
    let (xfered_value, xfered_unit) = pacman_humanize(xfered as f64);
    let (rate_value, rate_unit) = pacman_humanize(rate.trunc());
    let bar = fill_progress(percent, cols as i64 - infolen as i64);
    format!(
        "\r {fitted} {xfered_value:>6.1} {xfered_unit:>3}  {} {rate_unit:>3}/s {}{bar}",
        format_rate(rate_value),
        format_eta(eta),
    )
}

pub struct ConsoleSink {
    last_progress: Option<(ProgressPhase, String, i32)>,
    hook_phase: Option<bool>,
    color: bool,
    stderr_color: bool,
    downloads: DownloadBars,
    cursor: Cursor,
}

impl ConsoleSink {
    pub fn new() -> Self {
        Self {
            last_progress: None,
            hook_phase: None,
            color: color::stdout_color(),
            stderr_color: color::stderr_color(),
            downloads: DownloadBars::new(),
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
            InstallEvent::SyncDatabases => println!(
                "{}",
                color::colon(self.color, "Synchronizing package databases...")
            ),
            InstallEvent::StartSysupgrade => println!(
                "{}",
                color::colon(self.color, "Starting full system upgrade...")
            ),
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
                self.downloads.init_file(filename, Instant::now());
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
            InstallEvent::DownloadRetry { filename, .. } => {
                self.downloads.retry_file(filename, Instant::now());
            }
            InstallEvent::DownloadCompleted {
                filename,
                total,
                result,
            } => {
                self.download_completed(filename, *total, *result);
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
            InstallEvent::HookStart { pre } => {
                if let Some(label) = hook_header(&mut self.hook_phase, *pre) {
                    println!("{}", color::colon(self.color, label));
                }
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
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
            InstallEvent::ResolveDepsDone
            | InstallEvent::CheckDepsDone
            | InstallEvent::InterConflictsDone
            | InstallEvent::FileConflictsDone
            | InstallEvent::IntegrityDone
            | InstallEvent::LoadDone
            | InstallEvent::DiskSpaceDone
            | InstallEvent::KeyringDone
            | InstallEvent::KeyDownloadDone
            | InstallEvent::RetrieveStart
            | InstallEvent::RetrieveDone
            | InstallEvent::RetrieveFailed
            | InstallEvent::PkgRetrieveDone { .. }
            | InstallEvent::PkgRetrieveFailed { .. }
            | InstallEvent::PackageOperationEnd { .. }
            | InstallEvent::HookDone { .. }
            | InstallEvent::HookRunDone => {}
            InstallEvent::KeyDownloadStart => {
                println!(
                    "{}",
                    color::colon(self.color, "downloading required keys...")
                );
            }
            InstallEvent::OptDepRemoval { package, optdep } => {
                println!(
                    "{}",
                    color::colon(
                        self.color,
                        &format!("{package} optionally requires {optdep}")
                    )
                );
            }
            InstallEvent::DatabaseMissing { dbname } => {
                eprintln!(
                    "{} database file for '{dbname}' does not exist (use '-Sy' to download)",
                    color::paint(self.stderr_color, color::YELLOW, "warning:")
                );
            }
            InstallEvent::PacnewCreated { file, .. } => {
                eprintln!(
                    "{} {}",
                    color::paint(self.stderr_color, color::YELLOW, "warning:"),
                    crate::utils::pacnew_warning(file)
                );
            }
            InstallEvent::PacsaveCreated { file } => {
                eprintln!(
                    "{} {}",
                    color::paint(self.stderr_color, color::YELLOW, "warning:"),
                    crate::utils::pacsave_warning(file)
                );
            }
            InstallEvent::RuntimePrompt { .. } => {}
            InstallEvent::FailClosed { reason, .. } => {
                eprintln!(
                    "{} {}",
                    color::paint(self.stderr_color, color::RED, "error:"),
                    reason
                );
            }
        }
    }

    fn download_progress(&mut self, filename: &str, downloaded: i64, total: i64) {
        let drawn = self.downloads.progress_line(
            filename,
            downloaded,
            total,
            Instant::now(),
            crate::utils::terminal_cols(),
        );
        if let Some(line) = drawn {
            self.hide_cursor();
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
    }

    fn download_completed(&mut self, filename: &str, total: i64, result: DownloadResult) {
        match result {
            DownloadResult::UpToDate => {
                println!("{}", self.downloads.up_to_date_line(filename, self.color));
            }
            DownloadResult::Success => {
                println!(
                    "{}",
                    self.downloads.success_line(
                        filename,
                        total,
                        Instant::now(),
                        crate::utils::terminal_cols()
                    )
                );
            }
            DownloadResult::Failed => {
                println!("{}", self.downloads.failed_line(filename, self.color));
            }
        }
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
    hook_phase: Option<bool>,
    downloads: DownloadBars,
}

impl EscalatedSink {
    pub fn new() -> Self {
        EscalatedSink {
            hook_phase: None,
            downloads: DownloadBars::new(),
        }
    }
}

impl InstallSink for EscalatedSink {
    fn event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::HookStart { pre } => {
                if let Some(label) = hook_header(&mut self.hook_phase, pre) {
                    eprintln!("{}", color::colon(color::stderr_color(), label));
                }
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
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
            InstallEvent::DownloadInit { filename, optional } => {
                self.downloads.init_file(&filename, Instant::now());
                if optional {
                    eprintln!("  {filename} (optional)");
                }
            }
            InstallEvent::DownloadProgress {
                filename,
                downloaded,
                total,
            } => {
                let drawn = self.downloads.progress_line(
                    &filename,
                    downloaded,
                    total,
                    Instant::now(),
                    crate::utils::terminal_cols(),
                );
                if let Some(line) = drawn {
                    eprint!("{line}");
                    let _ = std::io::stderr().flush();
                }
            }
            InstallEvent::DownloadRetry { filename, .. } => {
                self.downloads.retry_file(&filename, Instant::now());
            }
            InstallEvent::DownloadCompleted {
                filename,
                total,
                result,
            } => match result {
                DownloadResult::UpToDate => {
                    eprintln!(
                        "{}",
                        self.downloads
                            .up_to_date_line(&filename, color::stderr_color())
                    );
                }
                DownloadResult::Success => {
                    eprintln!(
                        "{}",
                        self.downloads.success_line(
                            &filename,
                            total,
                            Instant::now(),
                            crate::utils::terminal_cols()
                        )
                    );
                }
                DownloadResult::Failed => {
                    eprintln!(
                        "{}",
                        self.downloads.failed_line(&filename, color::stderr_color())
                    );
                }
            },
            InstallEvent::SyncDatabases => {
                eprintln!(
                    "{}",
                    color::colon(color::stderr_color(), "Synchronizing package databases...")
                );
            }
            InstallEvent::StartSysupgrade => {
                eprintln!(
                    "{}",
                    color::colon(color::stderr_color(), "Starting full system upgrade...")
                );
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
            InstallEvent::OptDepRemoval { package, optdep } => {
                eprintln!("{package} optionally requires {optdep}");
            }
            InstallEvent::DatabaseMissing { dbname } => {
                eprintln!(
                    "{} database file for '{dbname}' does not exist (use '-Sy' to download)",
                    color::paint(color::stderr_color(), color::YELLOW, "warning:")
                );
            }
            InstallEvent::PacnewCreated { file, .. } => {
                eprintln!(
                    "{} {}",
                    color::paint(color::stderr_color(), color::YELLOW, "warning:"),
                    crate::utils::pacnew_warning(&file)
                );
            }
            InstallEvent::PacsaveCreated { file } => {
                eprintln!(
                    "{} {}",
                    color::paint(color::stderr_color(), color::YELLOW, "warning:"),
                    crate::utils::pacsave_warning(&file)
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

fn hook_header(phase: &mut Option<bool>, pre: bool) -> Option<&'static str> {
    if *phase == Some(pre) {
        return None;
    }
    *phase = Some(pre);
    Some(hook_phase_label(pre))
}

fn hook_phase_label(pre: bool) -> &'static str {
    if pre {
        "Running pre-transaction hooks..."
    } else {
        "Running post-transaction hooks..."
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
    let end = name
        .find(".pkg")
        .or_else(|| name.find(".db"))
        .or_else(|| name.find(".files"))
        .unwrap_or(name.len());
    &name[..end]
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
    fn hook_header_prints_pre_label_before_post() {
        let mut phase = None;
        assert_eq!(
            hook_header(&mut phase, true),
            Some("Running pre-transaction hooks...")
        );
        assert_eq!(hook_header(&mut phase, true), None);
        assert_eq!(
            hook_header(&mut phase, false),
            Some("Running post-transaction hooks...")
        );
    }

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
    fn download_progress_first_sample_shows_zero_rate_unknown_eta() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            xfered: 0,
            total: 0,
            init_time: base,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        };
        assert!(stat.observe(base, 1000, 9000));
        assert!(stat.rate > 0.0);
        assert_eq!(stat.rate.trunc(), 0.0);
        assert_eq!(stat.eta, ETA_UNKNOWN);
        assert_eq!(stat.sync_xfered, 1000);
        assert_eq!(stat.sync_time, Some(base));
    }

    #[test]
    fn download_progress_first_sample_of_complete_file_shows_zero_eta() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            xfered: 0,
            total: 0,
            init_time: base,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        };
        assert!(stat.observe(base, 742, 742));
        assert_eq!(stat.eta, 0);
    }

    #[test]
    fn download_progress_first_sample_of_empty_file_shows_unknown_eta() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            xfered: 0,
            total: 0,
            init_time: base,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        };
        assert!(stat.observe(base, 0, 0));
        assert_eq!(stat.rate, 0.0);
        assert_eq!(stat.eta, ETA_UNKNOWN);
    }

    #[test]
    fn download_progress_second_sample_computes_rate_and_eta() {
        let base = Instant::now();
        let mut stat = DownloadStat {
            xfered: 0,
            total: 0,
            init_time: base,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        };
        assert!(stat.observe(base, 1000, 9000));
        assert!(!stat.observe(base + Duration::from_millis(100), 2000, 9000));
        assert!(stat.observe(base + Duration::from_millis(300), 3000, 9000));
        let chunk_rate = 2000.0 * 1000.0 / 300.0;
        let expected_rate = (chunk_rate + 2.0 * 0.0) / 3.0;
        assert_eq!(stat.rate, expected_rate);
        assert_eq!(stat.eta, ((9000 - 3000) as f64 / expected_rate) as u64);
        assert_eq!(stat.sync_xfered, 3000);
        assert_eq!(stat.sync_time, Some(base + Duration::from_millis(300)));
    }

    #[test]
    fn clean_pkg_filename_strips_suffixes_in_pacman_priority() {
        assert_eq!(
            clean_pkg_filename("foo-1.2-3-x86_64.pkg.tar.zst"),
            "foo-1.2-3-x86_64"
        );
        assert_eq!(clean_pkg_filename("core.db"), "core");
        assert_eq!(clean_pkg_filename("core.db.sig"), "core");
        assert_eq!(clean_pkg_filename("extra.files"), "extra");
        assert_eq!(clean_pkg_filename("plain"), "plain");
    }

    #[test]
    fn pacman_humanize_uses_2048_threshold() {
        assert_eq!(pacman_humanize(0.0), (0.0, "B"));
        assert_eq!(pacman_humanize(2048.0), (2048.0, "B"));
        assert_eq!(pacman_humanize(2049.0), (2049.0 / 1024.0, "KiB"));
        assert_eq!(pacman_humanize(2048.0 * 1024.0), (2048.0, "KiB"));
    }

    #[test]
    fn draw_download_bar_matches_pacman_progress_line() {
        assert_eq!(
            draw_download_bar("core", 512, 1024, 0.0, ETA_UNKNOWN, 80),
            "\r core                  512.0   B  0.00   B/s --:-- [###########-----------]  50%"
        );
    }

    #[test]
    fn success_line_redraws_final_hundred_percent_bar() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_file("core.db", base);
        assert_eq!(
            bars.success_line("core.db", 2048, base + Duration::from_millis(1000), 80),
            "\r core                 2048.0   B  2048   B/s 00:01 [######################] 100%"
        );
    }

    #[test]
    fn completed_lines_match_pacman_up_to_date_and_failed() {
        let mut bars = DownloadBars::new();
        assert_eq!(
            bars.up_to_date_line("core.db", false),
            " core is up to date"
        );
        assert_eq!(
            bars.up_to_date_line("core.db", true),
            " core is up to date\x1b[K"
        );
        assert_eq!(
            bars.failed_line("core.db", false),
            " core.db failed to download"
        );
    }

    #[test]
    fn retry_resets_rate_and_eta_to_first_sample() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_file("core.db", base);
        let first = bars.progress_line("core.db", 1000, 9000, base, 80);
        assert!(first.is_some());
        let second =
            bars.progress_line("core.db", 3000, 9000, base + Duration::from_millis(300), 80);
        let second = second.expect("second sample redraws");
        assert!(second.contains("00:02"));
        bars.retry_file("core.db", base + Duration::from_millis(400));
        let after_retry =
            bars.progress_line("core.db", 500, 9000, base + Duration::from_millis(500), 80);
        let after_retry = after_retry.expect("post-retry sample redraws");
        assert!(after_retry.contains("0.00   B/s --:--"));
    }
}
