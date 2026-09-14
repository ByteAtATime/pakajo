use crate::color;
use crate::dispatch::child::{Presentation, code_from};
use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem};
use crate::dispatch::operation::{BuildOperation, ChildOperation, PrivilegedOperation};
use crate::dispatch::protocol::TerminalDecider;
use crate::events::{InstallEvent, InstallSink};
use crate::install::InstallTarget;
use clap::Parser;
use futures::StreamExt as _;

mod args;
use self::args::{
    CleanArgs, CompletionsArgs, InfoArgs, InstallArgs, RemoveArgs, SearchArgs, UpgradeArgs,
};
pub use self::args::{Cli, Command};

mod summary;

mod info;

pub(crate) mod prompts;
pub(crate) use self::prompts::{confirm_install, confirm_install_stderr};
pub(crate) use self::prompts::{confirm_remove, confirm_remove_stderr};

pub(crate) mod review;

mod chomp;
mod sinks;
mod spinner;
pub use self::sinks::ConsoleSink;
pub(crate) use self::sinks::{EscalatedSink, JsonSink};

pub(crate) mod privs;
use self::privs::{is_root, stdin_is_tty};

mod commands;
pub(crate) use self::commands::{alpm_handle, answerer_for};
use self::commands::{run_aur_sync, run_gendb, run_search};

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
        Some(Command::Install(a)) => std::process::exit(install_subcommand(a)),
        Some(Command::Remove(a)) => std::process::exit(remove_subcommand(a)),
        Some(Command::Upgrade(a)) => std::process::exit(upgrade_subcommand(a)),
        Some(Command::Search(a)) => search_subcommand(a),
        Some(Command::Info(a)) => info_subcommand(a),
        Some(Command::Clean(a)) => clean_subcommand(a),
        Some(Command::AurSync) => aur_sync_subcommand(),
        Some(Command::Gendb) => gendb_subcommand(),
        Some(Command::Completions(a)) => completions_subcommand(a),
        None => {}
    }
}

fn drain(mut stream: DispatchStream, json: bool) -> ChildOutcome {
    let mut sink = sink_for(json);
    while let Some(item) = futures::executor::block_on(stream.next()) {
        match item {
            StreamItem::Event(event) => sink.event(event),
            StreamItem::Done(outcome) => return outcome,
        }
    }
    ChildOutcome::Failed("stream ended".to_string())
}

fn outcome_code(outcome: &ChildOutcome) -> i32 {
    if matches!(outcome, ChildOutcome::Success) {
        return 0;
    }
    eprintln!("{}", outcome.reason());
    1
}

fn drain_privileged(operation: PrivilegedOperation, json: bool) -> ChildOutcome {
    let tty = stdin_is_tty();
    drain(operation.dispatch(tty), json)
}

fn drain_build(targets: &[String], as_deps: bool, json: bool, skip_review: bool) -> ChildOutcome {
    let operation = BuildOperation {
        targets: targets.to_vec(),
        as_deps,
    };
    let decider = Box::new(TerminalDecider::new(json, skip_review));
    drain(operation.dispatch(decider, None, stdin_is_tty()), json)
}

fn install_subcommand(args: InstallArgs) -> i32 {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        usage_error();
    }

    let handle = alpm_handle_or_exit();
    let positionals = expand_groups(&handle, &positionals, stdin_is_tty() && !args.json);

    if is_root() {
        let operation = ChildOperation::Install {
            targets: positionals,
            as_deps: args.as_deps,
            approvals_path: None,
            stream: args.json,
        };
        return code_from(operation.execute());
    }

    let (repo_or_file, aur) = split_install_targets(&handle, &positionals);

    if aur.is_empty() {
        install_repo_only(&handle, &positionals, args.as_deps, args.json)
    } else if repo_or_file.is_empty() {
        install_aur_only(&aur, args.as_deps, args.json, args.skip_review)
    } else {
        install_mixed(
            &handle,
            &repo_or_file,
            &aur,
            args.as_deps,
            args.json,
            args.skip_review,
        )
    }
}

fn install_repo_only(
    handle: &alpm::Alpm,
    positionals: &[String],
    as_deps: bool,
    json: bool,
) -> i32 {
    if !json {
        print_sync_preamble(handle, positionals);
    }
    let operation = PrivilegedOperation::Install {
        targets: positionals.to_vec(),
        as_deps,
        approvals: None,
    };
    outcome_code(&drain_privileged(operation, json))
}

fn install_aur_only(aur: &[String], as_deps: bool, json: bool, skip_review: bool) -> i32 {
    outcome_code(&drain_build(aur, as_deps, json, skip_review))
}

fn install_mixed(
    handle: &alpm::Alpm,
    repo_or_file: &[String],
    aur: &[String],
    as_deps: bool,
    json: bool,
    skip_review: bool,
) -> i32 {
    if !json {
        print_sync_preamble(handle, repo_or_file);
    }
    let operation = PrivilegedOperation::Install {
        targets: repo_or_file.to_vec(),
        as_deps,
        approvals: None,
    };
    let repo_outcome = drain_privileged(operation, json);
    if !matches!(repo_outcome, ChildOutcome::Success) {
        return outcome_code(&repo_outcome);
    }
    match drain_build(aur, as_deps, json, skip_review) {
        ChildOutcome::Success => 0,
        outcome => {
            let reason = outcome.reason();
            eprintln!("warning: repo packages installed; AUR phase failed: {reason}");
            1
        }
    }
}

fn search_subcommand(args: SearchArgs) -> ! {
    if args.query.is_empty() {
        eprintln!("usage: pakajo search <query>");
        std::process::exit(2);
    }
    let query = args.query.join(" ");
    exit_with_result(run_search(&query));
}

fn info_subcommand(args: InfoArgs) -> ! {
    info::run(args.targets)
}

fn aur_sync_subcommand() -> ! {
    exit_with_result(run_aur_sync());
}

fn gendb_subcommand() -> ! {
    exit_with_result(run_gendb());
}

fn completions_subcommand(args: CompletionsArgs) -> ! {
    exit_with_result(completions::run(args.shell));
}

fn clean_subcommand(args: CleanArgs) -> ! {
    exit_with_result(crate::clean::run_clean(args.remove));
}

fn remove_subcommand(args: RemoveArgs) -> i32 {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        eprintln!("usage: pakajo remove [--json] <package>...");
        std::process::exit(2);
    }

    let request = crate::dispatch::RemoveRequest {
        targets: positionals,
        tty: stdin_is_tty() && !args.json,
        json: args.json,
    };
    outcome_code(&drain(crate::dispatch::remove(request), args.json))
}

fn upgrade_subcommand(args: UpgradeArgs) -> i32 {
    if args.repo_only {
        let answerer = answerer_for(None);
        match crate::dispatch::child::select_upgrade_repo_presentation(args.json) {
            Presentation::SilentStream => {
                exit_with_result(crate::upgrade::run_repo_sysupgrade(
                    args.no_refresh,
                    &args.ignores,
                    JsonSink::new(),
                    answerer,
                    args.fingerprint_file.as_deref(),
                ));
            }
            Presentation::Console | Presentation::InteractiveStream => {
                exit_with_result(crate::upgrade::run_repo_sysupgrade(
                    args.no_refresh,
                    &args.ignores,
                    ConsoleSink::new(),
                    answerer,
                    args.fingerprint_file.as_deref(),
                ));
            }
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

    let operation = PrivilegedOperation::UpgradeRepo {
        no_refresh: args.no_refresh,
        ignores: args.ignores.clone(),
        fingerprint: None,
        approvals: None,
    };
    let repo_outcome = drain_privileged(operation, args.json);
    if !matches!(repo_outcome, ChildOutcome::Success) {
        return outcome_code(&repo_outcome);
    }
    if !aur_targets.is_empty() {
        let aur_names: Vec<String> = aur_targets.iter().map(|c| c.name.clone()).collect();
        match drain_build(&aur_names, false, args.json, args.skip_review) {
            ChildOutcome::Success => 0,
            outcome => {
                let reason = outcome.reason();
                eprintln!("warning: repo packages upgraded; AUR phase failed: {reason}");
                1
            }
        }
    } else {
        0
    }
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

pub(crate) fn classify_target(s: &str) -> InstallTarget {
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
