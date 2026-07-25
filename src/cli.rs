use std::io::Write as _;
use std::sync::Arc;

use anyhow::Context as _;

use crate::aur::AurClient;
use crate::color;
use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, ProgressPhase, SummaryPackage,
    TransactionSummary,
};
use crate::install::{self, InstallTarget};
use crate::search::{AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult};
use crate::utils::{format_bytes, format_mib};

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

    let mut repo_or_file: Vec<String> = Vec::new();
    let mut aur: Vec<String> = Vec::new();
    let handle = match alpm_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    for s in &positionals {
        match classify_target(s) {
            InstallTarget::File(_) => repo_or_file.push(s.clone()),
            InstallTarget::Repo(ref name) => {
                if crate::pacman::find_pkg(&handle, name).is_some() {
                    repo_or_file.push(s.clone());
                } else {
                    aur.push(s.clone());
                }
            }
        }
    }

    if aur.is_empty() {
        if !json {
            print_sync_preamble(&handle, &positionals);
        }
        escalate(&positionals, as_deps, json);
    } else if repo_or_file.is_empty() {
        let mut sink: Box<dyn InstallSink> = sink_for(json);
        if json {
            exit_with_result(crate::build::run_build(
                &aur,
                false,
                as_deps,
                &mut *sink,
                |_| true,
                approvals_b64.as_deref(),
            ));
        } else {
            exit_with_result(crate::build::run_build(
                &aur,
                false,
                as_deps,
                &mut *sink,
                confirm_build,
                approvals_b64.as_deref(),
            ));
        }
    } else {
        if !json {
            print_sync_preamble(&handle, &repo_or_file);
        }
        match escalate_result(&repo_or_file, as_deps, json) {
            Ok(0) => {}
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        let mut sink: Box<dyn InstallSink> = sink_for(json);
        let result = if json {
            crate::build::run_build(
                &aur,
                false,
                as_deps,
                &mut *sink,
                |_| true,
                approvals_b64.as_deref(),
            )
        } else {
            crate::build::run_build(
                &aur,
                false,
                as_deps,
                &mut *sink,
                confirm_build,
                approvals_b64.as_deref(),
            )
        };
        if let Err(e) = &result {
            eprintln!("warning: repo packages installed; AUR phase failed: {e:#}");
        }
        exit_with_result(result);
    }
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

pub(crate) fn gendb_subcommand(args: impl Iterator<Item = String>) -> ! {
    for s in args {
        if s.starts_with('-') {
            eprintln!("usage: pakajo gendb");
            std::process::exit(2);
        }
    }
    exit_with_result(run_gendb());
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

pub(crate) fn upgrade_subcommand(args: impl Iterator<Item = String>) -> ! {
    let mut args = args;
    let mut json = false;
    let mut no_refresh = false;
    let mut repo_only = false;
    let mut ignores: Vec<String> = Vec::new();
    while let Some(s) = args.next() {
        if s == "--json" {
            json = true;
        } else if s == "--no-refresh" {
            no_refresh = true;
        } else if s == "--repo-only" {
            repo_only = true;
        } else if s == "--ignore" {
            let v = args.next().unwrap_or_else(|| {
                eprintln!("--ignore requires a value");
                std::process::exit(2);
            });
            ignores.push(v);
        } else if let Some(rest) = s.strip_prefix("--ignore=") {
            ignores.push(rest.to_string());
        } else if s.starts_with('-') {
            eprintln!("unknown flag: {s}");
            std::process::exit(2);
        } else {
            eprintln!("usage: pakajo upgrade [--json] [--no-refresh]");
            std::process::exit(2);
        }
    }

    if repo_only {
        let answerer: Box<dyn crate::answerer::QuestionAnswerer> =
            if unsafe { libc::isatty(0) } == 1 {
                Box::new(crate::answerer::StdioAnswerer::new())
            } else {
                Box::new(crate::answerer::NonInteractiveAnswerer)
            };
        if json {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                no_refresh,
                &ignores,
                JsonSink::new(),
                answerer,
            ));
        } else {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                no_refresh,
                &ignores,
                ConsoleSink::new(),
                answerer,
            ));
        }
    }

    let handle = match alpm_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    let aur = crate::aur::AurClient::new();
    let mut aur_targets = match crate::upgrade::compute_aur_upgrades(&handle, &aur) {
        Ok(candidates) => candidates,
        Err(e) => {
            eprintln!("warning: AUR upgrade detection failed: {e:#}");
            vec![]
        }
    };
    aur_targets.retain(|c| !ignores.contains(&c.name));
    let mut sink = sink_for(json);
    sink.event(InstallEvent::SysupgradeAurCandidates {
        candidates: aur_targets.clone(),
    });

    let exit_code = escalate_upgrade(no_refresh, &ignores, json);
    if exit_code == 0 && !aur_targets.is_empty() {
        let aur_names: Vec<String> = aur_targets.iter().map(|c| c.name.clone()).collect();
        let mut build_sink: Box<dyn InstallSink> = sink_for(json);
        let result = if json {
            crate::build::run_build(&aur_names, false, false, &mut *build_sink, |_| true, None)
        } else {
            crate::build::run_build(
                &aur_names,
                false,
                false,
                &mut *build_sink,
                confirm_build,
                None,
            )
        };
        if let Err(e) = &result {
            eprintln!("warning: repo packages upgraded; AUR phase failed: {e:#}");
        }
        exit_with_result(result);
    }
    std::process::exit(exit_code);
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

fn escalate_upgrade(no_refresh: bool, ignores: &[String], json: bool) -> i32 {
    let mut child = match spawn_upgrade_child(no_refresh, ignores) {
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
    status.code().unwrap_or(1)
}

fn spawn_upgrade_child(
    no_refresh: bool,
    ignores: &[String],
) -> anyhow::Result<std::process::Child> {
    use std::process::Stdio;
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = escalation_command(&exe.to_string_lossy());
    cmd.arg("upgrade").arg("--json").arg("--repo-only");
    if no_refresh {
        cmd.arg("--no-refresh");
    }
    for name in ignores {
        cmd.arg("--ignore").arg(name);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn().context("failed to spawn upgrade child")
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

fn run_gendb() -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let arch = handle
        .architectures()
        .first()
        .context("no architecture configured in alpm")?;

    let sync_names: std::collections::HashSet<String> = handle
        .syncdbs()
        .iter()
        .flat_map(|db| db.pkgs().iter())
        .map(|p| p.name().to_string())
        .collect();
    let foreign: Vec<String> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .filter(|name| !sync_names.contains(name))
        .collect();

    let mut devel = crate::devel::load_devel_info();

    if foreign.is_empty() {
        crate::devel::save_devel_info(&devel)?;
        println!(
            "no foreign packages installed; wrote {}",
            crate::devel::state_path().display()
        );
        return Ok(());
    }

    let aur = crate::aur::AurClient::new();
    let infos = match aur.info_many(&foreign) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("warning: AUR info lookup failed: {e:#}");
            crate::devel::save_devel_info(&devel)?;
            return Ok(());
        }
    };

    let mut base_to_names: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for info in &infos {
        base_to_names
            .entry(info.package_base.clone())
            .or_default()
            .push(info.name.clone());
    }

    let mut recorded = 0usize;
    for (base, names) in &base_to_names {
        let pkg_info = match fetch_base_devel_info(base, arch) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("warning: skipping {base}: {e:#}");
                continue;
            }
        };
        for name in names {
            devel.info.insert(name.clone(), pkg_info.clone());
        }
        recorded += 1;
    }

    let path = crate::devel::state_path();
    crate::devel::save_devel_info(&devel)?;
    println!("recorded {recorded} devel package(s) to {}", path.display());
    Ok(())
}

fn fetch_base_devel_info(
    base: &str,
    arch: &str,
) -> anyhow::Result<Option<crate::devel::PkgInfo>> {
    let dir = crate::build::clone_dir(base)?;
    crate::build::git_clone_or_pull(&dir, base)?;
    let srcinfo = if dir.join(".SRCINFO").exists() {
        crate::srcinfo_io::read_from_dir(&dir)?
    } else {
        crate::srcinfo_io::generate(&dir)?
    };
    let pkg_info = crate::devel::fetch_devel_info(arch, &srcinfo)?;
    if pkg_info.repos.is_empty() {
        return Ok(None);
    }
    Ok(Some(pkg_info))
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
    let interactive = approvals.is_none() && unsafe { libc::isatty(0) } == 1;
    let answerer: Box<dyn crate::answerer::QuestionAnswerer> = if let Some(appr) = approvals {
        Box::new(crate::answerer::ApprovalsAnswerer::new(appr))
    } else if unsafe { libc::isatty(0) } == 1 {
        Box::new(crate::answerer::StdioAnswerer::new())
    } else {
        Box::new(crate::answerer::NonInteractiveAnswerer)
    };
    if json {
        if interactive {
            install::run_install(
                &targets,
                as_deps,
                EscalatedSink::new(),
                confirm_install_stderr,
                answerer,
            )
        } else {
            install::run_install(&targets, as_deps, JsonSink::new(), || true, answerer)
        }
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

fn sink_for(json: bool) -> Box<dyn InstallSink> {
    if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    }
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

fn print_sync_preamble(handle: &alpm::Alpm, targets: &[String]) {
    let labeled: Vec<String> = targets
        .iter()
        .map(|name| match crate::pacman::find_pkg(handle, name) {
            Some(pkg) => format!("{name}-{}", pkg.version()),
            None => name.clone(),
        })
        .collect();
    let c = color::stdout_color();
    println!(
        "{} {}",
        color::paint(c, color::BOLD, &format!("Sync Explicit ({}):", targets.len())),
        color::paint(c, color::CYAN, &labeled.join(", "))
    );
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
    last_progress: Option<(ProgressPhase, String, i32)>,
    hooks_header_done: bool,
    color: bool,
}

impl ConsoleSink {
    pub(crate) fn new() -> Self {
        Self {
            last_progress: None,
            hooks_header_done: false,
            color: color::stdout_color(),
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
                LogLevel::Error => eprint!("{} {message}", color::paint(self.color, color::RED, "error:")),
                LogLevel::Warning => eprint!(
                    "{} {message}",
                    color::paint(self.color, color::YELLOW, "warning:")
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
            InstallEvent::BuildOutput { line, .. } => println!("{line}"),
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

struct EscalatedSink;

impl EscalatedSink {
    fn new() -> Self {
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

fn confirm_install() -> bool {
    print!("\n");
    confirm_yes("Proceed with installation?")
}

fn confirm_remove() -> bool {
    print!("\n");
    confirm_yes("Proceed with removal?")
}

pub(crate) fn confirm_build(plan: &crate::resolve::BuildPlan) -> bool {
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
        .map(|(name, _, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let version_width = rows
        .iter()
        .map(|(_, version, _)| version.chars().count())
        .max()
        .unwrap_or(0);

    println!();
    let c = color::stdout_color();
    for (name, version, label) in &rows {
        let v = color::paint(c, color::VERSION, version);
        let vw = version_width + v.chars().count().saturating_sub(color::visible_width(&v));
        match label {
            Some(l) => println!(
                "  {:<nw$}  {:<vw$}  ({l})",
                name,
                v,
                nw = name_width,
                vw = vw,
            ),
            None => println!(
                "  {:<nw$}  {:<vw$}",
                name,
                v,
                nw = name_width,
                vw = vw,
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
        println!(
            "{}",
            color::colon(
                c,
                &format!("{aur_count} {aur_word} to build, {repo_dep_count} to install")
            )
        );
    } else {
        println!(
            "{}",
            color::colon(c, &format!("{aur_count} {aur_word} to build"))
        );
    }
    confirm_yes("Proceed with build?")
}

fn confirm_yes(message: &str) -> bool {
    let c = color::stdout_color();
    print!(
        "{} {} ",
        color::colon(c, message),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

fn confirm_install_stderr() -> bool {
    let c = color::stderr_color();
    eprint!(
        "\n{} {} ",
        color::colon(c, "Proceed with installation?"),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

fn print_summary(summary: &TransactionSummary) {
    if summary.packages.is_empty() {
        println!(" nothing to do");
        return;
    }
    print!("{}", render_summary(summary, color::stdout_color()));
}

struct SummaryColumn {
    header: String,
    right_align_data: bool,
    cells: Vec<String>,
}

fn append_table_line(out: &mut String, cells: &[String], right_align: &[bool], widths: &[usize]) {
    for (i, (cell, width)) in cells.iter().zip(widths.iter()).enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let target = *width + cell.chars().count().saturating_sub(color::visible_width(cell));
        if right_align[i] {
            out.push_str(&format!("{:>t$}", cell, t = target));
        } else {
            out.push_str(&format!("{:<t$}", cell, t = target));
        }
    }
    out.push('\n');
}

fn render_summary(summary: &TransactionSummary, colored: bool) -> String {
    let count = summary.packages.len();

    let mut ordered: Vec<&SummaryPackage> = summary.packages.iter().collect();
    ordered.sort_by_key(|p| (!p.is_removal, p.name.clone()));

    let rows: Vec<(String, String, String, String, String)> = ordered
        .iter()
        .map(|p| {
            let net = if p.is_removal {
                -p.installed_size
            } else {
                p.installed_size - p.old_installed_size
            };
            let dl = if p.download_size > 0 {
                format_mib(p.download_size)
            } else {
                String::new()
            };
            (
                formatted_name(p),
                p.old_version.clone().unwrap_or_default(),
                p.new_version.clone(),
                format_mib(net),
                dl,
            )
        })
        .collect();

    let has_old = rows.iter().any(|(_, old, _, _, _)| !old.is_empty());
    let has_new = rows.iter().any(|(_, _, new, _, _)| !new.is_empty());
    let has_dl = rows.iter().any(|(_, _, _, _, dl)| !dl.is_empty());

    let mut columns: Vec<SummaryColumn> = Vec::new();
    columns.push(SummaryColumn {
        header: format!("Package ({count})"),
        right_align_data: false,
        cells: rows.iter().map(|(name, _, _, _, _)| name.clone()).collect(),
    });
    if has_old {
        columns.push(SummaryColumn {
            header: "Old Version".to_string(),
            right_align_data: false,
            cells: rows.iter().map(|(_, old, _, _, _)| old.clone()).collect(),
        });
    }
    if has_new {
        columns.push(SummaryColumn {
            header: "New Version".to_string(),
            right_align_data: false,
            cells: rows.iter().map(|(_, _, new, _, _)| new.clone()).collect(),
        });
    }
    columns.push(SummaryColumn {
        header: "Net Change".to_string(),
        right_align_data: true,
        cells: rows.iter().map(|(_, _, _, net, _)| net.clone()).collect(),
    });
    if has_dl {
        columns.push(SummaryColumn {
            header: "Download Size".to_string(),
            right_align_data: true,
            cells: rows.iter().map(|(_, _, _, _, dl)| dl.clone()).collect(),
        });
    }

    let widths: Vec<usize> = columns
        .iter()
        .map(|col| {
            let mut w = col.header.len();
            for cell in &col.cells {
                w = w.max(cell.len());
            }
            w
        })
        .collect();

    let num_rows = rows.len();
    let mut out = String::new();

    out.push('\n');
    let header_cells: Vec<String> = columns
        .iter()
        .map(|c| color::paint(colored, color::BOLD, &c.header))
        .collect();
    let header_align: Vec<bool> = columns.iter().map(|_| false).collect();
    append_table_line(&mut out, &header_cells, &header_align, &widths);
    out.push('\n');
    let data_align: Vec<bool> = columns.iter().map(|c| c.right_align_data).collect();
    for row_idx in 0..num_rows {
        let row_cells: Vec<String> = columns.iter().map(|c| c.cells[row_idx].clone()).collect();
        append_table_line(&mut out, &row_cells, &data_align, &widths);
    }
    out.push('\n');

    append_footer(&mut out, summary, colored);

    out
}

fn append_footer(out: &mut String, summary: &TransactionSummary, colored: bool) {
    let dlsize = summary.total_download_size;
    let isize = summary.total_installed_size;
    let rsize = summary.total_removed_size;

    let mut rows: Vec<(String, String)> = Vec::new();
    if dlsize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Download Size:"),
            format_mib(dlsize),
        ));
    }
    if isize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Installed Size:"),
            format_mib(isize),
        ));
    }
    if rsize > 0 && isize == 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Removed Size:"),
            format_mib(rsize),
        ));
    }
    if isize > 0 && rsize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Net Upgrade Size:"),
            format_mib(isize - rsize),
        ));
    }
    if rows.is_empty() {
        return;
    }

    let lw = rows.iter().map(|(label, _)| color::visible_width(label)).max().unwrap_or(0);
    let vw = rows.iter().map(|(_, value)| color::visible_width(value)).max().unwrap_or(0);
    for (label, value) in &rows {
        let lwt = lw + label.chars().count().saturating_sub(color::visible_width(label));
        out.push_str(&format!("{:<lwt$}  {:>vw$}\n", label, value, lwt = lwt, vw = vw));
    }
}

fn formatted_name(pkg: &SummaryPackage) -> String {
    match &pkg.repository {
        Some(repo) => format!("{repo}/{}", pkg.name),
        None => pkg.name.clone(),
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

#[cfg(test)]
mod tests {
    use super::{classify_target, render_summary};
    use crate::events::{SummaryPackage, TransactionSummary};
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

    #[test]
    fn render_summary_matches_pacman_cava_case() {
        let installed = 199229;
        let removed = 241591;
        let summary = TransactionSummary {
            packages: vec![
                SummaryPackage {
                    name: "cava".to_string(),
                    repository: Some("extra".to_string()),
                    new_version: "0.10.7-1".to_string(),
                    old_version: None,
                    download_size: 0,
                    installed_size: installed,
                    old_installed_size: 0,
                    is_removal: false,
                },
                SummaryPackage {
                    name: "cava-git".to_string(),
                    repository: None,
                    new_version: String::new(),
                    old_version: Some("r1162.4b12c2b-1".to_string()),
                    download_size: 0,
                    installed_size: removed,
                    old_installed_size: 0,
                    is_removal: true,
                },
            ],
            total_download_size: 0,
            total_installed_size: installed,
            total_removed_size: removed,
        };

        let expected = [
            "",
            "Package (2)  Old Version      New Version  Net Change",
            "",
            "cava-git     r1162.4b12c2b-1                -0.23 MiB",
            "extra/cava                    0.10.7-1       0.19 MiB",
            "",
            "Total Installed Size:   0.19 MiB",
            "Net Upgrade Size:      -0.04 MiB",
        ]
        .join("\n")
            + "\n";

        let actual = render_summary(&summary, false);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }

    #[test]
    fn render_summary_matches_pacman_upgrade_case() {
        let new_isize = 1048576;
        let old_isize = 786432;
        let summary = TransactionSummary {
            packages: vec![SummaryPackage {
                name: "foo".to_string(),
                repository: Some("extra".to_string()),
                new_version: "2.0-1".to_string(),
                old_version: Some("1.0-1".to_string()),
                download_size: 0,
                installed_size: new_isize,
                old_installed_size: old_isize,
                is_removal: false,
            }],
            total_download_size: 0,
            total_installed_size: new_isize,
            total_removed_size: old_isize,
        };

        let expected = [
            "",
            "Package (1)  Old Version  New Version  Net Change",
            "",
            "extra/foo    1.0-1        2.0-1          0.25 MiB",
            "",
            "Total Installed Size:  1.00 MiB",
            "Net Upgrade Size:      0.25 MiB",
        ]
        .join("\n")
            + "\n";

        let actual = render_summary(&summary, false);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }
}
