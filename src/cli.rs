use std::io::Write as _;
use std::sync::Arc;

use anyhow::Context as _;

use crate::aur::AurClient;
use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};
use crate::install::{self, InstallTarget};
use crate::search::{AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult};
use crate::utils::format_bytes;

pub(crate) fn install_subcommand(args: impl Iterator<Item = String>) -> ! {
    let mut args = args;
    let mut json = false;
    let mut as_deps = false;
    let mut approvals_b64: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    while let Some(s) = args.next() {
        if s == "--json" {
            json = true;
        } else if s == "--asdeps" {
            as_deps = true;
        } else if s == "--approvals" {
            let v = args.next().unwrap_or_else(|| {
                eprintln!("--approvals requires a value");
                std::process::exit(2);
            });
            approvals_b64 = Some(v);
        } else if let Some(rest) = s.strip_prefix("--approvals=") {
            approvals_b64 = Some(rest.to_string());
        } else if s.starts_with('-') {
            eprintln!("unknown flag: {s}");
            std::process::exit(2);
        } else {
            positionals.push(s);
        }
    }

    let positionals = dedup_positionals(positionals);

    if positionals.is_empty() {
        usage_error();
    }

    if unsafe { libc::geteuid() } == 0 {
        let approvals = match approvals_b64.as_deref().map(decode_approvals).transpose() {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
        exit_with_result(root_install(&positionals, as_deps, json, approvals));
    }

    if positionals.len() == 1 {
        match classify_target(&positionals[0]) {
            InstallTarget::File(_) => escalate(&positionals, as_deps, json),
            InstallTarget::Repo(ref name) => {
                let exists = match repo_target_exists(name) {
                    Ok(exists) => exists,
                    Err(e) => {
                        eprintln!("{e:#}");
                        std::process::exit(1);
                    }
                };
                if exists {
                    escalate(&positionals, as_deps, json);
                }
                let mut sink: Box<dyn InstallSink> = if json {
                    Box::new(JsonSink::new())
                } else {
                    Box::new(ConsoleSink::new())
                };
                if json {
                    exit_with_result(crate::build::run_build(
                        std::slice::from_ref(name),
                        false,
                        as_deps,
                        &mut *sink,
                        |_| true,
                        approvals_b64.as_deref(),
                    ));
                } else {
                    exit_with_result(crate::build::run_build(
                        std::slice::from_ref(name),
                        false,
                        as_deps,
                        &mut *sink,
                        confirm_build,
                        approvals_b64.as_deref(),
                    ));
                }
            }
        }
    }

    escalate(&positionals, as_deps, json);
}

pub(crate) fn search_subcommand(args: impl Iterator<Item = String>) -> ! {
    let positionals: Vec<String> = args.filter(|s| !s.starts_with('-')).collect();
    if positionals.is_empty() {
        eprintln!("usage: pakajo search <query>");
        std::process::exit(2);
    }
    let query = positionals.join(" ");
    exit_with_result(run_search(&query));
}

pub(crate) fn aur_sync_subcommand(args: impl Iterator<Item = String>) -> ! {
    for s in args {
        if s.starts_with('-') {
            eprintln!("usage: pakajo aur-sync");
            std::process::exit(2);
        }
    }
    exit_with_result(run_aur_sync());
}

pub(crate) fn remove_subcommand(args: impl Iterator<Item = String>) -> ! {
    let mut json = false;
    let mut positionals: Vec<String> = Vec::new();
    for s in args {
        if s == "--json" {
            json = true;
        } else if s.starts_with('-') {
            eprintln!("unknown flag: {s}");
            std::process::exit(2);
        } else {
            positionals.push(s);
        }
    }

    let positionals = dedup_positionals(positionals);

    if positionals.is_empty() {
        eprintln!("usage: pakajo remove [--json] <package>...");
        std::process::exit(2);
    }

    if unsafe { libc::geteuid() } == 0 {
        let answerer: Box<dyn crate::answerer::QuestionAnswerer> =
            if unsafe { libc::isatty(0) } == 1 {
                Box::new(crate::answerer::StdioAnswerer::new())
            } else {
                Box::new(crate::answerer::NonInteractiveAnswerer)
            };
        if json {
            exit_with_result(crate::remove::run_remove(
                &positionals,
                JsonSink::new(),
                || true,
                answerer,
            ));
        } else {
            exit_with_result(crate::remove::run_remove(
                &positionals,
                ConsoleSink::new(),
                confirm_remove,
                answerer,
            ));
        }
    }

    escalate_remove(&positionals, json);
}

fn escalate_remove(targets: &[String], json: bool) -> ! {
    let mut child = match crate::remove::spawn_remove_child(targets) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink: Box<dyn InstallSink> = if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    crate::events::read_event_stream(std::io::BufReader::new(stdout), &mut *sink);
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(status.code().unwrap_or(1));
}

fn alpm_handle() -> anyhow::Result<alpm::Alpm> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    crate::pacman::init_alpm(&config)
}

fn run_aur_sync() -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let index = crate::local_index::LocalIndex::open(&crate::local_index::LocalIndex::db_path()?)?;
    match index.refresh(&handle)? {
        crate::local_index::RefreshOutcome::NotModified => println!("index up to date"),
        crate::local_index::RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        } => {
            println!("indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})");
        }
    }
    Ok(())
}

fn run_search(query: &str) -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let installed = crate::package::installed_names(&handle);
    let local_index = crate::local_index::LocalIndex::db_path()
        .ok()
        .and_then(|p| crate::local_index::LocalIndex::open(&p).ok())
        .map(Arc::new);
    let index = RepoSearchIndex::from_alpm(&handle);
    let repo_provider = RepoSearchProvider::new(Arc::new(index));
    let aur_provider = AurSearchProvider::new(Arc::new(AurClient::new()));
    let outcome = crate::search::dispatch_search(
        local_index,
        &repo_provider,
        &aur_provider,
        &installed,
        query,
    );
    print_search_results(&outcome.results);
    if let Some(err) = &outcome.aur_error {
        eprintln!("  aur: {err}");
    }
    Ok(())
}

fn print_search_results(rows: &[SearchResult]) {
    if rows.is_empty() {
        return;
    }
    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let version_width = rows.iter().map(|r| r.version.len()).max().unwrap_or(0);
    for row in rows {
        let repo = row.repo.as_deref().unwrap_or("-");
        let desc = row.description.as_deref().unwrap_or("-");
        println!(
            "  {:<nw$}  {:<vw$}  [{}]  {}",
            row.name,
            row.version,
            repo,
            desc,
            nw = name_width,
            vw = version_width,
        );
    }
}

fn decode_approvals(b64: &str) -> anyhow::Result<crate::question::Approvals> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("--approvals is not valid base64")?;
    serde_json::from_slice(&bytes).context("--approvals is not valid JSON")
}

fn root_install(
    positionals: &[String],
    as_deps: bool,
    json: bool,
    approvals: Option<crate::question::Approvals>,
) -> anyhow::Result<()> {
    let targets = positionals
        .iter()
        .map(|s| classify_target(s))
        .collect::<Vec<_>>();
    let needs_lookup = targets.iter().any(|t| matches!(t, InstallTarget::Repo(_)));
    if needs_lookup {
        let handle = alpm_handle()?;
        for target in &targets {
            if let InstallTarget::Repo(name) = target
                && crate::pacman::find_pkg(&handle, name).is_none()
            {
                anyhow::bail!("cannot build packages as root; re-run without privilege escalation");
            }
        }
    }
    let answerer: Box<dyn crate::answerer::QuestionAnswerer> = if let Some(appr) = approvals {
        Box::new(crate::answerer::ApprovalsAnswerer::new(appr))
    } else if unsafe { libc::isatty(0) } == 1 {
        Box::new(crate::answerer::StdioAnswerer::new())
    } else {
        Box::new(crate::answerer::NonInteractiveAnswerer)
    };
    if json {
        install::run_install(&targets, as_deps, JsonSink::new(), || true, answerer)
    } else {
        install::run_install(
            &targets,
            as_deps,
            ConsoleSink::new(),
            confirm_install,
            answerer,
        )
    }
}

fn escalate(targets: &[String], as_deps: bool, json: bool) -> ! {
    let code = match escalate_result(targets, as_deps, json) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(code);
}

fn escalate_result(targets: &[String], as_deps: bool, json: bool) -> anyhow::Result<i32> {
    let mut child = crate::build::spawn_install_child(targets, as_deps, None)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink: Box<dyn InstallSink> = if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    crate::events::read_event_stream(std::io::BufReader::new(stdout), &mut *sink);
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

fn repo_target_exists(name: &str) -> anyhow::Result<bool> {
    let handle = alpm_handle()?;
    Ok(crate::pacman::find_pkg(&handle, name).is_some())
}

pub(crate) fn escalation_command(exe: &str) -> std::process::Command {
    if unsafe { libc::geteuid() } == 0 {
        std::process::Command::new(exe)
    } else {
        let mut command = std::process::Command::new("pkexec");
        command.arg(exe);
        command
    }
}

fn classify_target(s: &str) -> InstallTarget {
    const FILE_SUFFIXES: &[&str] = &[".pkg.tar", ".pkg.tar.gz", ".pkg.tar.zst", ".pkg.tar.xz"];
    if FILE_SUFFIXES.iter().any(|suffix| s.ends_with(suffix)) {
        InstallTarget::File(s.into())
    } else {
        InstallTarget::Repo(s.into())
    }
}

fn dedup_positionals(positionals: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    positionals
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

fn usage_error() -> ! {
    eprintln!("usage: pakajo install [--json] <package>...");
    std::process::exit(2);
}

fn exit_with_result(result: anyhow::Result<()>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

pub(crate) struct ConsoleSink {
    to_build: usize,
    last_progress: Option<(ProgressPhase, String, i32)>,
}

impl ConsoleSink {
    pub(crate) fn new() -> Self {
        Self {
            to_build: 0,
            last_progress: None,
        }
    }

    fn print_event(&mut self, event: &InstallEvent) {
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
            InstallEvent::ResolvingAurDependencies { target } => {
                println!(":: resolving dependencies for {target}...");
            }
            InstallEvent::AurDepResolved { .. } => {}
            InstallEvent::ResolutionComplete { aur_packages, .. } => {
                self.to_build = *aur_packages;
            }
            InstallEvent::CloningRepo { package } => {
                println!(":: retrieving build files for {package}...");
            }
            InstallEvent::BuildStarted { package } => {
                println!(":: building {package}...");
            }
            InstallEvent::BuildOutput { package, line } => {
                if self.to_build > 1 {
                    println!("  [{package}] {line}");
                } else {
                    println!("  {line}");
                }
            }
            InstallEvent::BuildCompleted { package, .. } => {
                println!(":: built {package}");
            }
            InstallEvent::LayerBoundary { .. } => {}
        }
    }
}

impl InstallSink for ConsoleSink {
    fn event(&mut self, event: InstallEvent) {
        self.print_event(&event);
    }
}

struct JsonSink;

impl JsonSink {
    fn new() -> Self {
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

fn confirm_install() -> bool {
    confirm_yes("\n:: Proceed with installation?")
}

fn confirm_remove() -> bool {
    confirm_yes("\n:: Proceed with removal?")
}

fn confirm_build(plan: &crate::resolve::BuildPlan) -> bool {
    let rows: Vec<(&str, &str, Option<&str>)> = plan
        .layers
        .iter()
        .flat_map(|layer| layer.aur.iter())
        .map(|info| {
            let label = if plan.targets.iter().any(|t| t == &info.name) {
                None
            } else {
                Some("dependency")
            };
            (info.name.as_str(), info.version.as_str(), label)
        })
        .collect();

    let name_width = rows
        .iter()
        .map(|(name, _, _)| name.len())
        .max()
        .unwrap_or(0);
    let version_width = rows
        .iter()
        .map(|(_, version, _)| version.len())
        .max()
        .unwrap_or(0);

    println!();
    for (name, version, label) in &rows {
        match label {
            Some(l) => println!(
                "  {:<nw$}  {:<vw$}  ({l})",
                name,
                version,
                nw = name_width,
                vw = version_width,
            ),
            None => println!(
                "  {:<nw$}  {:<vw$}",
                name,
                version,
                nw = name_width,
                vw = version_width,
            ),
        }
    }
    println!();

    let aur_count = rows.len();
    let repo_dep_count: usize = plan.layers.iter().map(|l| l.repo_deps.len()).sum();
    let aur_word = if aur_count == 1 {
        "package"
    } else {
        "packages"
    };
    if repo_dep_count > 0 {
        println!(":: {aur_count} {aur_word} to build, {repo_dep_count} to install");
    } else {
        println!(":: {aur_count} {aur_word} to build");
    }
    confirm_yes(":: Proceed with build?")
}

fn confirm_yes(prompt: &str) -> bool {
    print!("{prompt} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
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
    match (&pkg.old_version, pkg.new_version.is_empty()) {
        (Some(old), false) => format!("{old} -> {}", pkg.new_version),
        _ => pkg
            .old_version
            .clone()
            .unwrap_or_else(|| pkg.new_version.clone()),
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

#[cfg(test)]
mod tests {
    use super::classify_target;
    use crate::install::InstallTarget;

    #[test]
    fn classify_target_table() {
        let cases: &[(&str, &str)] = &[
            ("sl", "repo"),
            ("google-chrome", "repo"),
            ("gtk3", "repo"),
            ("a-b_c", "repo"),
            ("foo.pkg.tar", "file"),
            ("foo.pkg.tar.gz", "file"),
            ("foo.pkg.tar.zst", "file"),
            ("foo.pkg.tar.xz", "file"),
            ("/tmp/foo-1.0-1-x86_64.pkg.tar.zst", "file"),
            ("./relative/bar.pkg.tar", "file"),
        ];
        for (input, expected) in cases {
            let actual = match classify_target(input) {
                InstallTarget::Repo(_) => "repo",
                InstallTarget::File(_) => "file",
            };
            assert_eq!(actual, *expected, "classify_target({input:?})");
        }
    }
}
