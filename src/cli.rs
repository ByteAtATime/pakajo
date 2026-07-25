use crate::color;
use crate::events::{InstallEvent, InstallSink};
use crate::install::InstallTarget;

mod args;
use self::args::{InstallArgs, RemoveArgs, UpgradeArgs};

mod summary;

mod prompts;
use self::prompts::{confirm_build, confirm_remove};

mod sinks;
pub(crate) use self::sinks::ConsoleSink;
use self::sinks::JsonSink;

mod privs;
use self::privs::{is_root, stdin_is_tty};

mod escalate;
use self::escalate::{escalate, escalate_remove, escalate_result, escalate_upgrade};
pub(crate) use self::escalate::escalation_command;

mod commands;
use self::commands::{alpm_handle, decode_approvals, root_install, run_aur_sync, run_gendb, run_search};

pub(crate) fn install_subcommand(args: impl Iterator<Item = String>) -> ! {
    let args = InstallArgs::parse(args);

    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        usage_error();
    }

    if is_root() {
        let approvals = match args.approvals_b64.as_deref().map(decode_approvals).transpose() {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
        exit_with_result(root_install(&positionals, args.as_deps, args.json, approvals));
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
        if !args.json {
            print_sync_preamble(&handle, &positionals);
        }
        escalate(&positionals, args.as_deps, args.json);
    } else if repo_or_file.is_empty() {
        let mut sink: Box<dyn InstallSink> = sink_for(args.json);
        if args.json {
            exit_with_result(crate::build::run_build(
                &aur,
                false,
                args.as_deps,
                &mut *sink,
                |_| true,
                args.approvals_b64.as_deref(),
            ));
        } else {
            exit_with_result(crate::build::run_build(
                &aur,
                false,
                args.as_deps,
                &mut *sink,
                confirm_build,
                args.approvals_b64.as_deref(),
            ));
        }
    } else {
        if !args.json {
            print_sync_preamble(&handle, &repo_or_file);
        }
        match escalate_result(&repo_or_file, args.as_deps, args.json) {
            Ok(0) => {}
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        let mut sink: Box<dyn InstallSink> = sink_for(args.json);
        let result = if args.json {
            crate::build::run_build(
                &aur,
                false,
                args.as_deps,
                &mut *sink,
                |_| true,
                args.approvals_b64.as_deref(),
            )
        } else {
            crate::build::run_build(
                &aur,
                false,
                args.as_deps,
                &mut *sink,
                confirm_build,
                args.approvals_b64.as_deref(),
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
    let args = RemoveArgs::parse(args);

    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        eprintln!("usage: pakajo remove [--json] <package>...");
        std::process::exit(2);
    }

    if is_root() {
        let answerer: Box<dyn crate::answerer::QuestionAnswerer> =
            if stdin_is_tty() {
                Box::new(crate::answerer::StdioAnswerer::new())
            } else {
                Box::new(crate::answerer::NonInteractiveAnswerer)
            };
        if args.json {
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

    escalate_remove(&positionals, args.json);
}

pub(crate) fn upgrade_subcommand(args: impl Iterator<Item = String>) -> ! {
    let args = UpgradeArgs::parse(args);

    if args.repo_only {
        let answerer: Box<dyn crate::answerer::QuestionAnswerer> =
            if stdin_is_tty() {
                Box::new(crate::answerer::StdioAnswerer::new())
            } else {
                Box::new(crate::answerer::NonInteractiveAnswerer)
            };
        if args.json {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                args.no_refresh,
                &args.ignores,
                JsonSink::new(),
                answerer,
            ));
        } else {
            exit_with_result(crate::upgrade::run_repo_sysupgrade(
                args.no_refresh,
                &args.ignores,
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
    aur_targets.retain(|c| !args.ignores.contains(&c.name));
    let mut sink = sink_for(args.json);
    sink.event(InstallEvent::SysupgradeAurCandidates {
        candidates: aur_targets.clone(),
    });

    let exit_code = escalate_upgrade(args.no_refresh, &args.ignores, args.json);
    if exit_code == 0 && !aur_targets.is_empty() {
        let aur_names: Vec<String> = aur_targets.iter().map(|c| c.name.clone()).collect();
        let mut build_sink: Box<dyn InstallSink> = sink_for(args.json);
        let result = if args.json {
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

fn sink_for(json: bool) -> Box<dyn InstallSink> {
    if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
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
