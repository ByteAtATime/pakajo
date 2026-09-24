use crate::dispatch::exec::{ChildOutcome, DispatchStream};
use crate::dispatch::protocol::{Decider, TerminalDecider};
use crate::events::InstallSink;
use crate::install::InstallTarget;
use clap::Parser;

mod args;
use self::args::{
    CleanArgs, CompletionsArgs, InfoArgs, InstallArgs, RemoveArgs, SearchArgs, UpgradeArgs,
};
pub use self::args::{Cli, Command};

pub(crate) mod summary;

mod info;

pub(crate) mod prompts;

pub(crate) mod review;

mod chomp;
mod sinks;
mod spinner;
pub use self::sinks::ConsoleSink;
pub(crate) use self::sinks::{EscalatedSink, JsonSink};

pub(crate) mod privs;
use self::privs::stdin_is_tty;

mod commands;
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

fn drain(stream: DispatchStream, json: bool) -> ChildOutcome {
    crate::dispatch::exec::drain_declining(stream, sink_for(json).as_mut())
}

fn outcome_code(outcome: &ChildOutcome) -> i32 {
    if matches!(
        outcome,
        ChildOutcome::Success | ChildOutcome::Stopped { idle: true }
    ) {
        return 0;
    }
    eprintln!("{}", outcome.reason());
    1
}

fn upgrade_subcommand(args: UpgradeArgs) -> i32 {
    let tty = stdin_is_tty() && !args.json;
    let request = crate::dispatch::SysupgradeRequest {
        no_refresh: args.no_refresh,
        repo_only: args.repo_only,
        ignores: args.ignores.clone(),
        decider: Box::new(TerminalDecider::new(args.json, args.skip_review, tty)),
        aur_targets: None,
        approvals: None,
        tty,
        json: args.json,
        print_nothing_to_do: true,
    };
    outcome_code(&drain(crate::dispatch::sysupgrade(request), args.json))
}

fn install_subcommand(args: InstallArgs) -> i32 {
    let positionals = args.positionals;

    if positionals.is_empty() {
        usage_error();
    }

    let stdin_tty = stdin_is_tty();
    let tty = stdin_tty && !args.json;
    let approvals = match piped_seal_payload(stdin_tty, std::io::stdin().lock()) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("{error:#}");
            return 1;
        }
    };
    let request = crate::dispatch::InstallRequest {
        targets: positionals,
        as_deps: args.as_deps,
        reinstall: args.reinstall,
        no_check: false,
        ignores: vec![],
        prefer_aur: false,
        decider: match decider_for(tty, args.json, args.skip_review, approvals.as_deref()) {
            Ok(decider) => decider,
            Err(error) => {
                eprintln!("{error:#}");
                return 1;
            }
        },
        approvals,
        tty,
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

    let stdin_tty = stdin_is_tty();
    let approvals = match piped_seal_payload(stdin_tty, std::io::stdin().lock()) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("{error:#}");
            return 1;
        }
    };
    let request = crate::dispatch::RemoveRequest {
        targets: positionals,
        tty: stdin_tty && !args.json,
        json: args.json,
        approvals,
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

pub(crate) const INVALID_PIPED_SEAL: &str = "seal payload from stdin is invalid";
pub(crate) const SEAL_PAYLOAD_TOO_LARGE: &str = "seal payload from stdin exceeds 1 MiB";
const MAX_SEAL_BYTES: u64 = 1024 * 1024;

pub(crate) fn sealed_decider_required(tty: bool, approvals: Option<&str>) -> bool {
    !tty && approvals.is_some()
}

pub(crate) fn decider_for(
    tty: bool,
    json: bool,
    skip_review: bool,
    approvals: Option<&str>,
) -> anyhow::Result<Box<dyn Decider + Send>> {
    if !sealed_decider_required(tty, approvals) {
        return Ok(Box::new(TerminalDecider::new(json, skip_review, tty)));
    }
    let payload = approvals.expect("sealed decider requires approvals");
    let sealed = crate::dispatch::seal::decode_seal(payload)
        .map_err(|error| anyhow::anyhow!("{INVALID_PIPED_SEAL}: {error:#}"))?;
    Ok(Box::new(crate::dispatch::seal::sealed_decider(sealed)))
}

pub(crate) fn piped_seal_payload(
    stdin_tty: bool,
    reader: impl std::io::Read,
) -> anyhow::Result<Option<String>> {
    if stdin_tty {
        return Ok(None);
    }
    read_piped_payload(reader)
}

fn read_piped_payload(reader: impl std::io::Read) -> anyhow::Result<Option<String>> {
    let payload = read_fully(reader)?;
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    crate::dispatch::seal::decode_seal(trimmed)
        .map_err(|error| anyhow::anyhow!("{INVALID_PIPED_SEAL}: {error:#}"))?;
    Ok(Some(trimmed.to_string()))
}

fn read_fully(reader: impl std::io::Read) -> anyhow::Result<String> {
    use std::io::Read as _;
    let mut payload = String::new();
    reader
        .take(MAX_SEAL_BYTES + 1)
        .read_to_string(&mut payload)
        .map_err(|error| anyhow::anyhow!("failed to read seal payload from stdin: {error:#}"))?;
    if payload.len() as u64 > MAX_SEAL_BYTES {
        anyhow::bail!("{SEAL_PAYLOAD_TOO_LARGE}");
    }
    Ok(payload)
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
    use super::{INVALID_PIPED_SEAL, SEAL_PAYLOAD_TOO_LARGE, classify_target, piped_seal_payload};
    use crate::cli::args::{Cli, Command};
    use crate::dispatch::seal::json_seal_missing;
    use crate::install::InstallTarget;
    use clap::Parser as _;
    use std::io::Cursor;

    #[test]
    fn install_reinstall_flag_defaults_off_and_parses() {
        let cli = Cli::try_parse_from(["pakajo", "install", "sl"]).unwrap();
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(!args.reinstall);
        let cli = Cli::try_parse_from(["pakajo", "install", "--reinstall", "sl"]).unwrap();
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.reinstall);
    }

    #[test]
    fn piped_seal_payload_attaches_non_empty_input() {
        let sealed = crate::dispatch::seal::proceed_only_seal().expect("encodes seal");
        let payload =
            piped_seal_payload(false, Cursor::new(sealed.clone())).expect("valid seal accepted");
        assert_eq!(payload, Some(sealed.clone()));
        let trailed = piped_seal_payload(false, Cursor::new(format!("{sealed}\n")))
            .expect("trailing newline accepted");
        assert_eq!(trailed, Some(sealed));
    }

    #[test]
    fn piped_seal_payload_empty_stdin_yields_no_approvals() {
        assert_eq!(
            piped_seal_payload(false, Cursor::new(String::new())).expect("empty accepted"),
            None
        );
        assert_eq!(
            piped_seal_payload(false, Cursor::new("  \n\t ")).expect("blank accepted"),
            None
        );
    }

    #[test]
    fn piped_seal_payload_never_reads_from_tty() {
        struct ExplodingReader;
        impl std::io::Read for ExplodingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                panic!("tty stdin must never be read");
            }
        }
        assert_eq!(
            piped_seal_payload(true, ExplodingReader).expect("tty yields none"),
            None
        );
    }

    #[test]
    fn piped_seal_payload_rejects_garbage_loudly() {
        let error =
            piped_seal_payload(false, Cursor::new("not a seal")).expect_err("garbage rejected");
        assert!(
            error.to_string().contains(INVALID_PIPED_SEAL),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn piped_seal_payload_satisfies_json_guard() {
        let sealed = crate::dispatch::seal::proceed_only_seal().expect("encodes seal");
        let approvals =
            piped_seal_payload(false, Cursor::new(sealed)).expect("valid seal accepted");
        assert!(!json_seal_missing(true, approvals.as_deref()));
        assert!(json_seal_missing(true, None));
    }
    #[test]
    fn piped_seal_payload_rejects_over_limit_input_loudly() {
        let oversized = "x".repeat(1024 * 1024 + 1);
        let error =
            piped_seal_payload(false, Cursor::new(oversized)).expect_err("over-limit rejected");
        assert!(
            error.to_string().contains(SEAL_PAYLOAD_TOO_LARGE),
            "unexpected error: {error:#}"
        );
    }

    fn conflict_seal_payload(incoming: &str, removable: &str, remove: bool) -> String {
        use crate::question::approvals::seal;
        use crate::question::model::{Answer, Question};
        let sealed = seal(
            &[Question::Conflict {
                incoming: incoming.to_string(),
                removable: removable.to_string(),
            }],
            &[Answer::Conflict {
                incoming: incoming.to_string(),
                removable: removable.to_string(),
                remove,
            }],
            true,
        )
        .expect("seal succeeds");
        crate::dispatch::seal::encode_seal(&sealed).expect("encodes")
    }

    fn conflicted_report() -> crate::resolve::ConflictReport {
        crate::resolve::ConflictReport {
            local: vec![crate::resolve::Conflict {
                pkg: "cava-git".to_string(),
                conflicting: vec![crate::resolve::Conflicting {
                    pkg: "cava".to_string(),
                    conflict: Some("cava".to_string()),
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn decider_for_keeps_terminal_on_tty_despite_seal() {
        use super::decider_for;
        use crate::build::BuildDecision;
        use crate::resolve::Plan;
        let sealed = conflict_seal_payload("cava-git", "cava", true);
        let decider = decider_for(true, true, false, Some(sealed.as_str())).expect("decider");
        assert_eq!(
            decider.confirm_build(&Plan::default()),
            BuildDecision::Review
        );
        assert!(!decider.confirm_conflicts(&conflicted_report()));
    }

    #[test]
    fn decider_for_honors_sealed_conflict_answer_without_json() {
        use super::decider_for;
        use crate::build::BuildDecision;
        use crate::resolve::Plan;
        for json in [false, true] {
            let approving = conflict_seal_payload("cava-git", "cava", true);
            let decider =
                decider_for(false, json, false, Some(approving.as_str())).expect("decider");
            assert_eq!(
                decider.confirm_build(&Plan::default()),
                BuildDecision::Proceed
            );
            assert!(decider.confirm_conflicts(&conflicted_report()));
            assert!(decider.review_pkgbuilds(&[]));
            let refusing = conflict_seal_payload("cava-git", "cava", false);
            let decider =
                decider_for(false, json, false, Some(refusing.as_str())).expect("decider");
            assert!(!decider.confirm_conflicts(&conflicted_report()));
        }
    }

    #[test]
    fn decider_for_rejects_corrupt_seal_loudly() {
        use super::{INVALID_PIPED_SEAL, decider_for};
        let error = match decider_for(false, false, false, Some("not a seal")) {
            Ok(_) => panic!("corrupt seal accepted"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(INVALID_PIPED_SEAL),
            "unexpected error: {error:#}"
        );
    }

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
