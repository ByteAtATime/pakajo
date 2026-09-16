use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem};
use crate::dispatch::protocol::TerminalDecider;
use crate::events::InstallSink;
use crate::install::InstallTarget;
use anyhow::Context as _;
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
pub(crate) use self::prompts::{PromptStream, confirm_hold_remove};
pub(crate) use self::prompts::{confirm_install, confirm_install_stderr};
pub(crate) use self::prompts::{confirm_remove, confirm_remove_stderr};

pub(crate) mod review;

mod chomp;
mod sinks;
mod spinner;
pub use self::sinks::ConsoleSink;
pub(crate) use self::sinks::{EscalatedSink, JsonSink};

pub(crate) mod privs;
use self::privs::stdin_is_tty;

mod commands;
pub(crate) use self::commands::answerer_for;
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

fn upgrade_subcommand(args: UpgradeArgs) -> i32 {
    let fingerprint = match args.fingerprint_file.as_deref().map(seal_fingerprint_file) {
        Some(Ok(file)) => Some(file),
        Some(Err(error)) => {
            eprintln!("{error:#}");
            return 1;
        }
        None => None,
    };
    let request = crate::dispatch::SysupgradeRequest {
        no_refresh: args.no_refresh,
        repo_only: args.repo_only,
        ignores: args.ignores.clone(),
        decider: Box::new(TerminalDecider::new(args.json, args.skip_review)),
        aur_targets: None,
        fingerprint,
        approvals: None,
        tty: stdin_is_tty() && !args.json,
        json: args.json,
    };
    outcome_code(&drain(crate::dispatch::sysupgrade(request), args.json))
}

fn seal_fingerprint_file(path: &str) -> anyhow::Result<crate::dispatch::approvals::ApprovalsFile> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read fingerprint file {path}"))?;
    crate::dispatch::approvals::ApprovalsFile::write(&bytes)
}

fn install_subcommand(args: InstallArgs) -> i32 {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        usage_error();
    }

    let request = crate::dispatch::InstallRequest {
        targets: positionals,
        as_deps: args.as_deps,
        ignores: vec![],
        prefer_aur: false,
        decider: Box::new(TerminalDecider::new(args.json, args.skip_review)),
        approvals: None,
        tty: stdin_is_tty() && !args.json,
        json: args.json,
    };
    outcome_code(&drain(crate::dispatch::install(request), args.json))
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

fn sink_for(json: bool) -> Box<dyn InstallSink> {
    if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    }
}

pub(crate) fn classify_target(s: &str) -> InstallTarget {
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

fn alpm_handle_or_exit() -> alpm::Alpm {
    match crate::pacman::handle() {
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
