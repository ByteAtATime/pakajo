use crate::color;
use crate::events::{InstallEvent, InstallSink};
use crate::install::InstallTarget;
use clap::Parser;

mod args;
use self::args::{
    CleanArgs, CompletionsArgs, InfoArgs, InstallArgs, RemoveArgs, SearchArgs, UpgradeArgs,
};
pub use self::args::{Cli, Command};

mod summary;

mod info;

mod prompts;
use self::prompts::{
    confirm_build, confirm_proceed_to_review, confirm_remove, confirm_remove_stderr,
};

mod review;

mod chomp;
mod sinks;
mod spinner;
pub use self::sinks::ConsoleSink;
use self::sinks::{EscalatedSink, JsonSink};

mod privs;
use self::privs::{is_root, stdin_is_tty};

mod escalate;
use self::escalate::{escalate, escalate_remove, escalate_result, escalate_upgrade};
pub use self::escalate::{escalation_command, graphical_escalation_command};

mod commands;
use self::commands::{
    alpm_handle, answerer_for, decode_approvals, root_install, run_aur_sync, run_gendb, run_search,
};

mod complete;
mod completions;

pub fn parse() -> Cli {
    let mut argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("__complete") => exit_with_result(complete::run(&argv[2..])),
        Some("-S") => argv[1] = "install".to_string(),
        Some("-R") => argv[1] = "remove".to_string(),
        _ => {}
    }
    Cli::parse_from(argv)
}

pub fn dispatch(cli: Cli) {
    match cli.command {
        Some(Command::Install(a)) => install_subcommand(a),
        Some(Command::Remove(a)) => remove_subcommand(a),
        Some(Command::Upgrade(a)) => upgrade_subcommand(a),
        Some(Command::Search(a)) => search_subcommand(a),
        Some(Command::Info(a)) => info_subcommand(a),
        Some(Command::Clean(a)) => clean_subcommand(a),
        Some(Command::AurSync) => aur_sync_subcommand(),
        Some(Command::Gendb) => gendb_subcommand(),
        Some(Command::Completions(a)) => completions_subcommand(a),
        None => {}
    }
}

pub fn install_subcommand(args: InstallArgs) -> ! {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        usage_error();
    }

    let handle = alpm_handle_or_exit();
    let positionals = expand_groups(&handle, &positionals, stdin_is_tty() && !args.json);

    if is_root() {
        let approvals = decode_approvals_or_exit(args.approvals_b64.as_deref());
        exit_with_result(root_install(
            &handle,
            &positionals,
            args.as_deps,
            args.json,
            approvals,
        ));
    }

    let (repo_or_file, aur) = split_install_targets(&handle, &positionals);

    if aur.is_empty() {
        install_repo_only(
            &handle,
            &positionals,
            args.as_deps,
            args.json,
            args.approvals_b64.as_deref(),
        );
    } else if repo_or_file.is_empty() {
        install_aur_only(
            &aur,
            args.as_deps,
            args.json,
            args.skip_review,
            args.approvals_b64.as_deref(),
        );
    } else {
        install_mixed(
            &handle,
            &repo_or_file,
            &aur,
            args.as_deps,
            args.json,
            args.skip_review,
            args.approvals_b64.as_deref(),
        );
    }
}

fn install_repo_only(
    handle: &alpm::Alpm,
    positionals: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> ! {
    if !json {
        print_sync_preamble(handle, positionals);
    }
    escalate(positionals, as_deps, json, approvals_b64);
}

fn install_aur_only(
    aur: &[String],
    as_deps: bool,
    json: bool,
    skip_review: bool,
    approvals_b64: Option<&str>,
) -> ! {
    let mut sink: Box<dyn InstallSink> = sink_for(json);
    let callbacks = build_callbacks(json, skip_review);
    exit_with_result(crate::build::run_build(
        aur,
        false,
        as_deps,
        &mut *sink,
        callbacks.confirm,
        callbacks.review,
        approvals_b64,
    ));
}

fn install_mixed(
    handle: &alpm::Alpm,
    repo_or_file: &[String],
    aur: &[String],
    as_deps: bool,
    json: bool,
    skip_review: bool,
    approvals_b64: Option<&str>,
) -> ! {
    if !json {
        print_sync_preamble(handle, repo_or_file);
    }
    match escalate_result(repo_or_file, as_deps, json, approvals_b64) {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
    let mut sink: Box<dyn InstallSink> = sink_for(json);
    let callbacks = build_callbacks(json, skip_review);
    let result = crate::build::run_build(
        aur,
        false,
        as_deps,
        &mut *sink,
        callbacks.confirm,
        callbacks.review,
        approvals_b64,
    );
    if let Err(e) = &result {
        eprintln!("warning: repo packages installed; AUR phase failed: {e:#}");
    }
    exit_with_result(result);
}

pub fn search_subcommand(args: SearchArgs) -> ! {
    if args.query.is_empty() {
        eprintln!("usage: pakajo search <query>");
        std::process::exit(2);
    }
    let query = args.query.join(" ");
    exit_with_result(run_search(&query));
}

pub fn info_subcommand(args: InfoArgs) -> ! {
    info::run(args.targets)
}

pub fn aur_sync_subcommand() -> ! {
    exit_with_result(run_aur_sync());
}

pub fn gendb_subcommand() -> ! {
    exit_with_result(run_gendb());
}

pub fn completions_subcommand(args: CompletionsArgs) -> ! {
    exit_with_result(completions::run(args.shell));
}

pub fn clean_subcommand(args: CleanArgs) -> ! {
    exit_with_result(crate::clean::run_clean(args.remove));
}

pub fn remove_subcommand(args: RemoveArgs) -> ! {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        eprintln!("usage: pakajo remove [--json] <package>...");
        std::process::exit(2);
    }

    let handle = alpm_handle_or_exit();
    let positionals = expand_remove_groups(&handle, &positionals, stdin_is_tty() && !args.json);

    if is_root() {
        let interactive = stdin_is_tty();
        let answerer = answerer_for(None);
        if args.json {
            if interactive {
                exit_with_result(crate::remove::run_remove(
                    &positionals,
                    EscalatedSink::new(),
                    confirm_remove_stderr,
                    answerer,
                ));
            } else {
                exit_with_result(crate::remove::run_remove(
                    &positionals,
                    JsonSink::new(),
                    || true,
                    answerer,
                ));
            }
        } else {
            exit_with_result(crate::remove::run_remove(
                &positionals,
                ConsoleSink::new(),
                confirm_remove,
                answerer,
            ));
        }
    }

    escalate_remove(&positionals, args.json);
}

pub fn upgrade_subcommand(args: UpgradeArgs) -> ! {
    let approvals = decode_approvals_or_exit(args.approvals_b64.as_deref());
    if args.repo_only {
        let answerer = answerer_for(approvals);
        if args.json {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                args.no_refresh,
                &args.ignores,
                JsonSink::new(),
                answerer,
                args.fingerprint_file.as_deref(),
            ));
        } else {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                args.no_refresh,
                &args.ignores,
                ConsoleSink::new(),
                answerer,
                args.fingerprint_file.as_deref(),
            ));
        }
    }

    let handle = alpm_handle_or_exit();
    let aur = crate::aur::AurClient::new();
    let mut aur_targets = match crate::upgrade::compute_aur_upgrades(
        &handle,
        &aur,
        crate::upgrade::DevelSource::Live,
    ) {
        Ok((candidates, _)) => candidates,
        Err(e) => {
            eprintln!("warning: AUR upgrade detection failed: {e:#}");
            vec![]
        }
    };
    aur_targets.retain(|c| !args.ignores.contains(&c.name));
    let mut sink = sink_for(args.json);
    sink.event(InstallEvent::SysupgradeAurCandidates {
        candidates: aur_targets.clone(),
    });

    let exit_code = escalate_upgrade(args.no_refresh, &args.ignores, args.json);
    if exit_code == 0 && !aur_targets.is_empty() {
        let aur_names: Vec<String> = aur_targets.iter().map(|c| c.name.clone()).collect();
        let mut build_sink: Box<dyn InstallSink> = sink_for(args.json);
        let callbacks = build_callbacks(args.json, args.skip_review);
        let result = crate::build::run_build(
            &aur_names,
            false,
            false,
            &mut *build_sink,
            callbacks.confirm,
            callbacks.review,
            None,
        );
        if let Err(e) = &result {
            eprintln!("warning: repo packages upgraded; AUR phase failed: {e:#}");
        }
        exit_with_result(result);
    }
    std::process::exit(exit_code);
}

struct BuildCallbacks<C> {
    confirm: C,
    review: ReviewCallback,
}

type ReviewCallback = fn(&[crate::pkgbuild::PkgbuildInfo]) -> bool;

fn build_callbacks(
    json: bool,
    skip_review: bool,
) -> BuildCallbacks<impl FnOnce(&crate::resolve::BuildPlan) -> crate::build::BuildDecision> {
    let confirm = move |plan: &crate::resolve::BuildPlan| -> crate::build::BuildDecision {
        if json {
            crate::build::BuildDecision::Review
        } else if skip_review {
            confirm_build(plan)
        } else {
            confirm_proceed_to_review(plan)
        }
    };
    let review: ReviewCallback = if json {
        |_| true
    } else {
        self::review::review_pkgbuilds
    };
    BuildCallbacks { confirm, review }
}

fn sink_for(json: bool) -> Box<dyn InstallSink> {
    if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    }
}

fn expand_groups(handle: &alpm::Alpm, positionals: &[String], interactive: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in positionals {
        if crate::package::repo_exists(handle, s) {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        let groups = crate::package::find_groups(handle, s);
        if groups.is_empty() {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        let members: Vec<String> = if interactive {
            self::prompts::select_group_members(s, &groups)
        } else {
            groups
                .iter()
                .flat_map(|g| g.members.iter())
                .map(|m| m.name.clone())
                .collect()
        };
        for name in members {
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

fn expand_remove_groups(
    handle: &alpm::Alpm,
    positionals: &[String],
    interactive: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in positionals {
        if crate::package::is_installed(handle, s) {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        if let Some(group) = crate::package::local_group(handle, s) {
            let members: Vec<String> = if interactive {
                self::prompts::select_group_members(s, std::slice::from_ref(&group))
            } else {
                group.members.iter().map(|m| m.name.clone()).collect()
            };
            for name in members {
                if seen.insert(name.clone()) {
                    out.push(name);
                }
            }
            continue;
        }
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

fn classify_target(s: &str) -> InstallTarget {
    const FILE_SUFFIXES: &[&str] = &[".pkg.tar", ".pkg.tar.gz", ".pkg.tar.zst", ".pkg.tar.xz"];
    if FILE_SUFFIXES.iter().any(|suffix| s.ends_with(suffix)) {
        InstallTarget::File(s.into())
    } else {
        InstallTarget::Repo(s.into())
    }
}

fn split_install_targets(
    handle: &alpm::Alpm,
    positionals: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut repo_or_file: Vec<String> = Vec::new();
    let mut aur: Vec<String> = Vec::new();
    for s in positionals {
        match classify_target(s) {
            InstallTarget::File(_) => repo_or_file.push(s.clone()),
            InstallTarget::Repo(ref name) => {
                if crate::package::repo_exists(handle, name) {
                    repo_or_file.push(s.clone());
                } else {
                    aur.push(s.clone());
                }
            }
        }
    }
    (repo_or_file, aur)
}

fn print_sync_preamble(handle: &alpm::Alpm, targets: &[String]) {
    let labeled: Vec<String> = targets
        .iter()
        .map(|name| match crate::package::find(handle, name) {
            Some(pkg) => format!("{name}-{}", pkg.version),
            None => name.clone(),
        })
        .collect();
    let c = color::stdout_color();
    println!(
        "{} {}",
        color::paint(
            c,
            color::BOLD,
            &format!("Sync Explicit ({}):", targets.len())
        ),
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

fn alpm_handle_or_exit() -> alpm::Alpm {
    match alpm_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

fn decode_approvals_or_exit(approvals_b64: Option<&str>) -> Option<crate::question::Approvals> {
    match approvals_b64.map(decode_approvals).transpose() {
        Ok(opt) => opt,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
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
