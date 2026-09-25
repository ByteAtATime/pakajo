use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::time::Instant;

use super::summary::{print_summary, render_summary};
use crate::color::{self, HIDE_CURSOR, SHOW_CURSOR};
use crate::download::{FileTransfer, Meter, Totals};
use crate::events::{DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase};
use crate::utils::{format_eta, format_rate, terminal_cols};

#[derive(Clone, Copy)]
enum Stream {
    Out,
    Err,
}

impl Stream {
    fn print(self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self {
            Stream::Out => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            Stream::Err => {
                eprint!("{text}");
                let _ = std::io::stderr().flush();
            }
        }
    }

    fn println(self, text: &str) {
        match self {
            Stream::Out => println!("{text}"),
            Stream::Err => eprintln!("{text}"),
        }
    }

    fn colon(self, colored: bool, msg: &str) {
        self.println(&color::colon(colored, msg));
    }

    fn warning(self, colored: bool, msg: &str) {
        self.println(&format!(
            "{} {msg}",
            color::paint(colored, color::YELLOW, "warning:")
        ));
    }

    fn error(self, colored: bool, msg: &str) {
        self.println(&format!(
            "{} {msg}",
            color::paint(colored, color::RED, "error:")
        ));
    }
}

struct DownloadBars {
    files: HashMap<String, FileTransfer>,
    order: Vec<String>,
    finished: HashSet<String>,
    cursor: i64,
    totals: Totals,
    draw_total: bool,
}

impl DownloadBars {
    fn new() -> Self {
        DownloadBars {
            files: HashMap::new(),
            order: Vec::new(),
            finished: HashSet::new(),
            cursor: 0,
            totals: Totals::default(),
            draw_total: false,
        }
    }

    fn draw_total(&self, cols: usize, colored: bool) -> String {
        if !self.draw_total {
            return String::new();
        }
        meter_bar(
            &total_label(self.totals.downloaded, self.totals.howmany),
            &self.totals.meter,
            cols,
            colored,
        )
    }

    fn init_total(&mut self, num: usize, total_bytes: i64, now: Instant, color: bool) {
        self.totals.reset(num, total_bytes, now);
        self.draw_total = color && num > 1 && total_bytes > 0;
    }

    fn move_to(&mut self, index: i64, color: bool) -> String {
        if !color {
            return String::new();
        }
        let delta = index - self.cursor;
        self.cursor = index;
        if delta > 0 {
            format!("\x1B[{delta}E")
        } else if delta < 0 {
            format!("\x1B[{}F", -delta)
        } else {
            String::new()
        }
    }

    fn move_end(&mut self, color: bool) -> String {
        self.move_to(self.order.len() as i64, color)
    }

    fn move_below_total(&mut self, color: bool) -> String {
        let end = self.order.len() as i64 + i64::from(self.draw_total);
        self.move_to(end, color)
    }

    fn claim_line(&mut self, filename: &str, color: bool) -> String {
        if !color || self.order.iter().any(|name| name == filename) {
            return String::new();
        }
        let mut out = self.move_end(true);
        out.push_str(&format!(" {}\n", clean_pkg_filename(filename)));
        self.order.push(filename.to_string());
        self.cursor += 1;
        out
    }

    fn bar_index(&mut self, filename: &str) -> i64 {
        if let Some(index) = self.order.iter().position(|name| name == filename) {
            return index as i64;
        }
        self.order.push(filename.to_string());
        self.order.len() as i64 - 1
    }

    fn release_head(&mut self) {
        while self
            .order
            .first()
            .is_some_and(|name| self.finished.contains(name))
        {
            if let Some(name) = self.order.first().cloned() {
                self.finished.remove(&name);
            }
            self.order.remove(0);
            self.cursor -= 1;
        }
    }

    fn init_file(&mut self, filename: &str, now: Instant, cols: usize, color: bool) -> String {
        self.files
            .insert(filename.to_string(), FileTransfer::fresh(now));
        let mut out = self.claim_line(filename, color);
        if !color || !self.draw_total {
            return out;
        }
        out.push_str(&self.move_to(self.order.len() as i64, true));
        out.push_str(&self.draw_total(cols, color));
        out.push('\n');
        self.cursor += 1;
        out
    }

    fn progress_line(
        &mut self,
        filename: &str,
        downloaded: i64,
        total: i64,
        now: Instant,
        cols: usize,
        color: bool,
    ) -> Option<String> {
        let previous = self.files.get(filename).map_or(0, |file| file.downloaded);
        let chunk = downloaded - previous;
        let file_drew = self
            .files
            .entry(filename.to_string())
            .or_insert_with(|| FileTransfer::fresh(now))
            .observe(now, downloaded, total);
        let mut out = String::new();
        if file_drew {
            let file = self.files.get(filename).expect("file observed");
            let line = meter_bar(clean_pkg_filename(filename), &file.meter, cols, color);
            if !color {
                return Some(line);
            }
            out.push_str(&self.claim_line(filename, true));
            let index = self.bar_index(filename);
            out.push_str(&self.move_to(index, true));
            out.push_str(&line);
        }
        if !color || !self.draw_total {
            return (!out.is_empty()).then_some(out);
        }
        if self.totals.add_chunk(chunk, now) {
            out.push_str(&self.move_to(self.order.len() as i64, true));
            out.push_str(&self.draw_total(cols, color));
        }
        (!out.is_empty()).then_some(out)
    }

    fn retry_file(&mut self, filename: &str, now: Instant, resume: bool) {
        let previous = self.files.get(filename).map_or(0, |file| file.downloaded);
        if let Some(file) = self.files.get_mut(filename) {
            file.reset(now);
        }
        if !resume {
            self.totals.rollback(previous);
        }
    }

    fn status_line(&mut self, filename: &str, message: &str, color: bool) -> String {
        self.files.remove(filename);
        self.totals.count_completion();
        let clear = if color { "\x1b[K" } else { "" };
        if !color {
            return format!(" {message}{clear}");
        }
        let index = self.bar_index(filename);
        let mut out = self.move_to(index, true);
        out.push_str(&format!(" {message}{clear}"));
        self.finished.insert(filename.to_string());
        self.release_head();
        out
    }

    fn success_line(
        &mut self,
        filename: &str,
        total: i64,
        now: Instant,
        cols: usize,
        color: bool,
    ) -> String {
        self.totals.count_completion();
        let file = self
            .files
            .entry(filename.to_string())
            .or_insert_with(|| FileTransfer::fresh(now));
        file.complete(total);
        file.meter.finish(now, total);
        let line = meter_bar(clean_pkg_filename(filename), &file.meter, cols, color);
        if !color {
            return line;
        }
        let index = self.bar_index(filename);
        let mut out = self.move_to(index, true);
        out.push_str(&line);
        self.finished.insert(filename.to_string());
        self.release_head();
        out
    }

    fn finish_total(&mut self, now: Instant, cols: usize, color: bool) -> String {
        if !color || !self.draw_total {
            self.draw_total = false;
            return String::new();
        }
        self.totals.finish(now);
        let index = self.order.len() as i64;
        let mut out = self.move_to(index, true);
        out.push_str(&self.draw_total(cols, color));
        out.push('\n');
        self.cursor = index;
        self.draw_total = false;
        out
    }

    fn complete_line(
        &mut self,
        filename: &str,
        total: i64,
        result: DownloadResult,
        now: Instant,
        cols: usize,
        color: bool,
    ) -> String {
        match result {
            DownloadResult::UpToDate => self.status_line(
                filename,
                &format!("{} is up to date", clean_pkg_filename(filename)),
                color,
            ),
            DownloadResult::Success => self.success_line(filename, total, now, cols, color),
            DownloadResult::Failed => {
                self.status_line(filename, &format!("{filename} failed to download"), color)
            }
        }
    }
}

fn is_download_event(event: &InstallEvent) -> bool {
    matches!(
        event,
        InstallEvent::DownloadInit { .. }
            | InstallEvent::DownloadProgress { .. }
            | InstallEvent::DownloadRetry { .. }
            | InstallEvent::DownloadCompleted { .. }
    )
}

fn move_below_event(
    downloads: &mut DownloadBars,
    event: &InstallEvent,
    stream: Stream,
    colored: bool,
) {
    let end = match event {
        InstallEvent::PkgRetrieveDone { .. } | InstallEvent::PkgRetrieveFailed { .. } => {
            downloads.move_end(colored)
        }
        _ => downloads.move_below_total(colored),
    };
    stream.print(&end);
}

fn download_event(
    downloads: &mut DownloadBars,
    event: &InstallEvent,
    stream: Stream,
    colored: bool,
    mut cursor: Option<&mut Cursor>,
) {
    let now = Instant::now();
    match event {
        InstallEvent::RetrievingPackages { num, total_bytes } => {
            downloads.init_total(*num, *total_bytes, now, colored);
            stream.colon(colored, "Retrieving packages...");
        }
        InstallEvent::DownloadInit { filename, optional } => {
            let claim = downloads.init_file(filename, now, terminal_cols(), colored);
            if !claim.is_empty() {
                if let Some(cursor) = cursor.as_deref_mut() {
                    cursor.hide();
                }
                stream.print(&claim);
            }
            if *optional {
                stream.println(&format!("  {filename} (optional)"));
            }
        }
        InstallEvent::DownloadProgress {
            filename,
            downloaded,
            total,
        } => {
            if let Some(line) = downloads.progress_line(
                filename,
                *downloaded,
                *total,
                now,
                terminal_cols(),
                colored,
            ) {
                if let Some(cursor) = cursor {
                    cursor.hide();
                }
                stream.print(&line);
            }
        }
        InstallEvent::DownloadRetry { filename, resume } => {
            downloads.retry_file(filename, now, *resume);
        }
        InstallEvent::DownloadCompleted {
            filename,
            total,
            result,
        } => {
            let line =
                downloads.complete_line(filename, *total, *result, now, terminal_cols(), colored);
            if colored {
                stream.print(&line);
            } else {
                stream.println(&line);
            }
        }
        InstallEvent::PkgRetrieveDone { .. } | InstallEvent::PkgRetrieveFailed { .. } => {
            stream.print(&downloads.finish_total(now, terminal_cols(), colored));
        }
        _ => {}
    }
}

fn total_label(downloaded: usize, howmany: usize) -> String {
    let width = count_digits(howmany);
    format!("Total ({downloaded:>width$}/{howmany:>width$})")
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

fn meter_bar(cleaned: &str, meter: &Meter, cols: usize, colored: bool) -> String {
    draw_download_bar(
        cleaned,
        meter.sync_xfered,
        meter.total,
        meter.rate,
        meter.eta,
        cols,
        colored,
    )
}

fn draw_download_bar(
    cleaned: &str,
    xfered: i64,
    total: i64,
    rate: f64,
    eta: u64,
    cols: usize,
    colored: bool,
) -> String {
    let percent = download_percent(xfered, total);
    let infolen = (cols * 6 / 10).max(50);
    let filenamelen = infolen.saturating_sub(30);
    let fitted = fit_subject(cleaned, filenamelen);
    let (xfered_value, xfered_unit) = pacman_humanize(xfered as f64);
    let (rate_value, rate_unit) = pacman_humanize(rate.trunc());
    let proglen = cols as i64 - infolen as i64;
    let mut bar = String::new();
    if proglen > 8 {
        bar.push(' ');
        bar.push_str(&super::chomp::render(
            percent,
            (proglen - 8) as usize,
            colored,
        ));
    }
    if proglen >= 5 {
        bar.push_str(&format!(" {percent:>3}%"));
    }
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

    fn say(&self, msg: &str) {
        println!("{}", color::colon(self.color, msg));
    }

    fn warn(&self, msg: &str) {
        Stream::Err.warning(self.stderr_color, msg);
    }

    fn fail(&self, msg: &str) {
        Stream::Err.error(self.stderr_color, msg);
    }

    fn print_event(&mut self, event: &InstallEvent) {
        let stream = Stream::Out;
        if is_download_event(event) {
            download_event(
                &mut self.downloads,
                event,
                stream,
                self.color,
                Some(&mut self.cursor),
            );
            return;
        }
        move_below_event(&mut self.downloads, event, stream, self.color);
        match event {
            InstallEvent::ResolvingDependencies => println!("resolving dependencies..."),
            InstallEvent::CheckingConflicts => println!("looking for conflicting packages..."),
            InstallEvent::CheckingDependencies => println!("checking dependencies..."),
            InstallEvent::LoadingPackages => println!("loading packages..."),
            InstallEvent::SyncDatabases => self.say("Synchronizing package databases..."),
            InstallEvent::StartSysupgrade => self.say("Starting full system upgrade..."),
            InstallEvent::RetrievingPackages { .. }
            | InstallEvent::PkgRetrieveDone { .. }
            | InstallEvent::PkgRetrieveFailed { .. } => {
                download_event(&mut self.downloads, event, stream, self.color, None);
            }
            InstallEvent::ProcessingChanges => self.say("Processing package changes..."),
            InstallEvent::WaitingForDatabaseLock => {
                self.say("Pacman is currently in use, please wait...")
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
                self.cursor.hide();
                print_progress(*phase, package, *percent, *current, *total, self.color);
            }
            InstallEvent::HookStart { pre } => {
                if let Some(label) = hook_header(&mut self.hook_phase, *pre) {
                    self.say(label);
                }
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => println!(
                "{}",
                hook_run_line(*position, *total, name, desc.as_deref())
            ),
            InstallEvent::ScriptletInfo { line } => {
                if line.ends_with('\n') {
                    print!("{line}");
                } else {
                    println!("{line}");
                }
            }
            InstallEvent::Log { level, message } => match level {
                LogLevel::Error => self.fail(message.trim_end()),
                LogLevel::Warning => self.warn(message.trim_end()),
                LogLevel::Debug => {}
            },
            InstallEvent::TransactionSummary(s) => print_summary(s),
            InstallEvent::ResolvingAurDependencies { target } => {
                self.say(&format!("resolving dependencies for {target}..."));
            }
            InstallEvent::BuildOutput { line, .. } => {
                if self.color {
                    println!("{line}");
                } else {
                    println!("{}", color::ansi_strip(line));
                }
            }
            InstallEvent::PkgbuildAllUpToDate { packages } => {
                let msg = if packages.len() == 1 {
                    format!("{}: already reviewed, no changes", packages[0])
                } else {
                    format!("{} packages already reviewed, no changes", packages.len())
                };
                self.say(&msg);
            }
            InstallEvent::SysupgradeAurCandidates { candidates } => {
                if candidates.is_empty() {
                    return;
                }
                self.say(&format!("{} AUR package(s) to upgrade:", candidates.len()));
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
            InstallEvent::KeyDownloadStart => self.say("downloading required keys..."),
            InstallEvent::OptDepRemoval { package, optdep } => {
                self.say(&format!("{package} optionally requires {optdep}"));
            }
            InstallEvent::DatabaseMissing { dbname } => self.warn(&format!(
                "database file for '{dbname}' does not exist (use '-Sy' to download)"
            )),
            InstallEvent::PacnewCreated { file, .. } => {
                self.warn(&crate::utils::pacnew_warning(file))
            }
            InstallEvent::PacsaveCreated { file } => {
                self.warn(&crate::utils::pacsave_warning(file))
            }
            InstallEvent::FailClosed { reason, .. } => self.fail(reason),
            _ => {}
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
        self.cursor.show();
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
        let stream = Stream::Err;
        let colored = color::stderr_color();
        if is_download_event(&event) {
            download_event(&mut self.downloads, &event, stream, colored, None);
            return;
        }
        move_below_event(&mut self.downloads, &event, stream, colored);
        match event {
            InstallEvent::HookStart { pre } => {
                if let Some(label) = hook_header(&mut self.hook_phase, pre) {
                    stream.colon(colored, label);
                }
            }
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => stream.println(&hook_run_line(position, total, &name, desc.as_deref())),
            InstallEvent::TransactionSummary(s) => {
                if s.packages.is_empty() {
                    stream.println(" nothing to do");
                } else {
                    stream.print(&render_summary(&s, colored));
                }
            }
            InstallEvent::CheckingDependencies => stream.println("checking dependencies..."),
            InstallEvent::RetrievingPackages { .. }
            | InstallEvent::PkgRetrieveDone { .. }
            | InstallEvent::PkgRetrieveFailed { .. } => {
                download_event(&mut self.downloads, &event, stream, colored, None);
            }
            InstallEvent::SyncDatabases => {
                stream.colon(colored, "Synchronizing package databases...")
            }
            InstallEvent::StartSysupgrade => {
                stream.colon(colored, "Starting full system upgrade...")
            }
            InstallEvent::WaitingForDatabaseLock => {
                stream.colon(colored, "Pacman is currently in use, please wait...")
            }
            InstallEvent::Log { level, message } => match level {
                LogLevel::Warning => stream.warning(colored, message.trim_end()),
                LogLevel::Error => stream.error(colored, message.trim_end()),
                LogLevel::Debug => {}
            },
            InstallEvent::OptDepRemoval { package, optdep } => {
                stream.println(&format!("{package} optionally requires {optdep}"));
            }
            InstallEvent::DatabaseMissing { dbname } => stream.warning(
                colored,
                &format!("database file for '{dbname}' does not exist (use '-Sy' to download)"),
            ),
            InstallEvent::PacnewCreated { file, .. } => {
                stream.warning(colored, &crate::utils::pacnew_warning(&file))
            }
            InstallEvent::PacsaveCreated { file } => {
                stream.warning(colored, &crate::utils::pacsave_warning(&file))
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

    fn hide(&mut self) {
        if self.color && !self.hidden {
            self.hidden = true;
            Stream::Out.print(HIDE_CURSOR);
        }
    }

    fn show(&mut self) {
        if self.hidden {
            self.hidden = false;
            Stream::Out.print(SHOW_CURSOR);
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
    let cols = terminal_cols();
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

    use crate::download::ETA_UNKNOWN;

    fn prog(bars: &mut DownloadBars, file: &str, dl: i64, t: i64, at: Instant) -> Option<String> {
        bars.progress_line(file, dl, t, at, 80, true)
    }

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
    fn fit_subject_truncates_with_ellipsis() {
        assert_eq!(fit_subject("hello world", 6), "hel...");
    }

    #[test]
    fn fit_subject_keeps_exact_fit() {
        assert_eq!(fit_subject("hello", 5), "hello");
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
            draw_download_bar("core", 512, 1024, 0.0, ETA_UNKNOWN, 80, false),
            "\r core                  512.0   B  0.00   B/s --:-- [-----------C o  o  o  ]  50%"
        );
    }

    #[test]
    fn success_line_redraws_final_hundred_percent_bar() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_file("core.db", base, 80, false);
        assert_eq!(
            bars.success_line(
                "core.db",
                2048,
                base + Duration::from_millis(1000),
                80,
                false
            ),
            "\r core                 2048.0   B  2048   B/s 00:01 [----------------------] 100%"
        );
    }

    #[test]
    fn interleaved_up_to_date_owns_its_line() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        assert_eq!(bars.init_file("a.db", base, 80, true), " a\n");
        assert_eq!(bars.init_file("b.db", base, 80, true), " b\n");
        assert_eq!(
            prog(&mut bars, "a.db", 0, 1000, base).as_deref(),
            Some(
                "\x1B[2F\r a                       0.0   B  0.00   B/s --:-- [\x1B[1;33mC\x1B[0m\x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  ]   0%"
            )
        );
        assert_eq!(
            bars.status_line("b.db", "b is up to date", true),
            "\x1B[1E b is up to date\x1b[K"
        );
        assert_eq!(
            prog(
                &mut bars,
                "a.db",
                500,
                1000,
                base + Duration::from_millis(300)
            )
            .as_deref(),
            Some(
                "\x1B[1F\r a                     500.0   B   555   B/s 00:00 [-----------\x1B[1;33mC\x1B[0m \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  ]  50%"
            )
        );
        assert_eq!(
            bars.success_line("a.db", 1000, base + Duration::from_millis(1000), 80, true),
            "\r a                    1000.0   B  1000   B/s 00:01 [----------------------] 100%"
        );
        assert_eq!(bars.move_end(true), "\x1B[2E");
    }

    #[test]
    fn sequential_color_claims_redraws_and_releases() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        assert_eq!(bars.init_file("a.db", base, 80, true), " a\n");
        assert_eq!(
            prog(&mut bars, "a.db", 0, 1000, base).as_deref(),
            Some(
                "\x1B[1F\r a                       0.0   B  0.00   B/s --:-- [\x1B[1;33mC\x1B[0m\x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  ]   0%"
            )
        );
        assert_eq!(
            bars.success_line("a.db", 1000, base + Duration::from_millis(1000), 80, true),
            "\r a                    1000.0   B  1000   B/s 00:01 [----------------------] 100%"
        );
        assert_eq!(bars.move_end(true), "\x1B[1E");
    }

    #[test]
    fn plain_mode_emits_no_cursor_protocol() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        assert_eq!(bars.init_file("a.db", base, 80, false), "");
        assert!(
            bars.progress_line("a.db", 0, 1000, base, 80, false)
                .expect("first sample draws")
                .starts_with("\r a")
        );
        assert_eq!(
            bars.status_line("b.db", "b is up to date", false),
            " b is up to date"
        );
        assert_eq!(bars.move_end(false), "");
    }

    #[test]
    fn completed_lines_match_pacman_up_to_date_and_failed() {
        let mut bars = DownloadBars::new();
        assert_eq!(
            bars.status_line("core.db", "core is up to date", false),
            " core is up to date"
        );
        assert_eq!(
            bars.status_line("core.db", "core is up to date", true),
            " core is up to date\x1b[K"
        );
        assert_eq!(
            bars.status_line("core.db", "core.db failed to download", false),
            " core.db failed to download"
        );
    }

    #[test]
    fn total_bar_two_inits_redraw_total_each_claim() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, true);
        let total = "\r Total (0/2)             0.0   B  0.00   B/s --:-- [\x1B[1;33mC\x1B[0m\x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  \x1B[0;37mo\x1B[0m  ]   0%\n";
        assert_eq!(
            bars.init_file("a.pkg.tar.zst", base, 80, true),
            format!(" a\n{total}")
        );
        assert_eq!(
            bars.init_file("b.pkg.tar.zst", base, 80, true),
            format!("\x1B[1F b\n{total}")
        );
    }

    #[test]
    fn total_label_pads_to_howmany_digits() {
        assert_eq!(total_label(1, 12), "Total ( 1/12)");
        assert_eq!(total_label(12, 12), "Total (12/12)");
        assert_eq!(total_label(0, 2), "Total (0/2)");
    }

    #[test]
    fn total_progress_accumulates_without_file_window() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, true);
        bars.init_file("a.pkg.tar.zst", base, 80, true);
        let first = bars
            .progress_line("a.pkg.tar.zst", 500, 1000, base, 80, true)
            .expect("total first sample draws");
        assert!(first.contains("Total (0/2)"));
        let quiet = prog(
            &mut bars,
            "a.pkg.tar.zst",
            700,
            1000,
            base + Duration::from_millis(100),
        );
        assert!(quiet.is_none());
        assert_eq!(bars.totals.xfered, 700);
        let later = prog(
            &mut bars,
            "a.pkg.tar.zst",
            800,
            1000,
            base + Duration::from_millis(300),
        );
        assert!(later.is_some());
    }

    #[test]
    fn total_retry_without_resume_decrements() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, true);
        bars.init_file("a.pkg.tar.zst", base, 80, true);
        prog(&mut bars, "a.pkg.tar.zst", 500, 1000, base);
        bars.retry_file("a.pkg.tar.zst", base + Duration::from_millis(50), false);
        assert_eq!(bars.totals.xfered, 0);
        prog(&mut bars, "a.pkg.tar.zst", 500, 1000, base);
        bars.retry_file("a.pkg.tar.zst", base + Duration::from_millis(60), true);
        assert_eq!(bars.totals.xfered, 500);
    }

    #[test]
    fn total_completion_counts_every_result_without_redraw() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(3, 3000, base, true);
        bars.init_file("a.pkg.tar.zst", base, 80, true);
        bars.init_file("b.pkg.tar.zst", base, 80, true);
        bars.init_file("c.pkg.tar.zst", base, 80, true);
        bars.complete_line("a.pkg.tar.zst", 0, DownloadResult::Success, base, 80, true);
        assert_eq!(bars.totals.downloaded, 1);
        bars.complete_line("b.pkg.tar.zst", 0, DownloadResult::UpToDate, base, 80, true);
        assert_eq!(bars.totals.downloaded, 2);
        bars.complete_line("c.pkg.tar.zst", 0, DownloadResult::Failed, base, 80, true);
        assert_eq!(bars.totals.downloaded, 3);
    }

    #[test]
    fn total_finish_draws_hundred_percent_and_deactivates() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, true);
        bars.init_file("a.pkg.tar.zst", base, 80, true);
        bars.init_file("b.pkg.tar.zst", base, 80, true);
        let done = bars.finish_total(base + Duration::from_millis(1000), 80, true);
        assert!(done.ends_with("\n"));
        assert!(done.contains("Total (0/2)"));
        assert!(done.contains("100%"));
        assert!(!bars.draw_total);
        let again = bars.finish_total(base + Duration::from_millis(2000), 80, true);
        assert_eq!(again, "");
    }

    #[test]
    fn total_disabled_for_single_package_or_empty_total() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(1, 2000, base, true);
        assert!(!bars.draw_total);
        bars.init_total(2, 0, base, true);
        assert!(!bars.draw_total);
        let claim = bars.init_file("a.pkg.tar.zst", base, 80, true);
        assert_eq!(claim, " a\n");
    }

    #[test]
    fn total_disabled_without_color() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, false);
        assert!(!bars.draw_total);
        let claim = bars.init_file("a.pkg.tar.zst", base, 80, false);
        assert_eq!(claim, "");
    }

    #[test]
    fn total_line_follows_head_pop() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_total(2, 2000, base, true);
        bars.init_file("a.pkg.tar.zst", base, 80, true);
        bars.init_file("b.pkg.tar.zst", base, 80, true);
        bars.complete_line("a.pkg.tar.zst", 0, DownloadResult::Success, base, 80, true);
        assert_eq!(bars.order.len(), 1);
        let drawn = prog(
            &mut bars,
            "b.pkg.tar.zst",
            400,
            1000,
            base + Duration::from_millis(500),
        );
        assert!(drawn.is_some());
    }

    #[test]
    fn retry_resets_rate_and_eta_to_first_sample() {
        let base = Instant::now();
        let mut bars = DownloadBars::new();
        bars.init_file("core.db", base, 80, false);
        let first = bars.progress_line("core.db", 1000, 9000, base, 80, false);
        assert!(first.is_some());
        let second = bars.progress_line(
            "core.db",
            3000,
            9000,
            base + Duration::from_millis(300),
            80,
            false,
        );
        let second = second.expect("second sample redraws");
        assert!(second.contains("00:02"));
        bars.retry_file("core.db", base + Duration::from_millis(400), false);
        let after_retry = bars.progress_line(
            "core.db",
            500,
            9000,
            base + Duration::from_millis(500),
            80,
            false,
        );
        let after_retry = after_retry.expect("post-retry sample redraws");
        assert!(after_retry.contains("0.00   B/s --:--"));
    }
}
