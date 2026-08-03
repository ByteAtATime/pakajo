use crate::color;
use crate::events::{InstallEvent, InstallSink};
use crate::install::InstallTarget;
use clap::Parser;

mod args;
use self::args::{CleanArgs, InstallArgs, RemoveArgs, SearchArgs, UpgradeArgs};
pub(crate) use self::args::{Cli, Command};

mod summary;

mod prompts;
use self::prompts::{
    confirm_build, confirm_proceed_to_review, confirm_remove, confirm_remove_stderr,
};

mod review;

mod sinks;
pub(crate) use self::sinks::ConsoleSink;
use self::sinks::{EscalatedSink, JsonSink};

mod privs;
use self::privs::{is_root, stdin_is_tty};

mod escalate;
pub(crate) use self::escalate::escalation_command;
use self::escalate::{escalate, escalate_remove, escalate_result, escalate_upgrade};

mod commands;
use self::commands::{
    alpm_handle, answerer_for, decode_approvals, root_install, run_aur_sync, run_gendb, run_search,
};

pub(crate) fn parse() -> Cli {
    let mut argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("-S") => argv[1] = "install".to_string(),
        Some("-R") => argv[1] = "remove".to_string(),
        _ => {}
    }
    Cli::parse_from(argv)
}

pub(crate) fn install_subcommand(args: InstallArgs) -> ! {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        usage_error();
    }

    if is_root() {
        let approvals = match args
            .approvals_b64
            .as_deref()
            .map(decode_approvals)
            .transpose()
        {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
        exit_with_result(root_install(
            &positionals,
            args.as_deps,
            args.json,
            approvals,
        ));
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
        escalate(
            &positionals,
            args.as_deps,
            args.json,
            args.approvals_b64.as_deref(),
        );
    } else if repo_or_file.is_empty() {
        let mut sink: Box<dyn InstallSink> = sink_for(args.json);
        let json = args.json;
        let skip_review = args.skip_review;
        let confirm = move |plan: &crate::resolve::BuildPlan| -> crate::build::BuildDecision {
            if json {
                crate::build::BuildDecision::Review
            } else if skip_review {
                confirm_build(plan)
            } else {
                confirm_proceed_to_review(plan)
            }
        };
        let review: fn(&[crate::pkgbuild::PkgbuildInfo]) -> bool = if json {
            |_| true
        } else {
            self::review::review_pkgbuilds
        };
        exit_with_result(crate::build::run_build(
            &aur,
            false,
            args.as_deps,
            &mut *sink,
            confirm,
            review,
            args.approvals_b64.as_deref(),
        ));
    } else {
        if !args.json {
            print_sync_preamble(&handle, &repo_or_file);
        }
        match escalate_result(
            &repo_or_file,
            args.as_deps,
            args.json,
            args.approvals_b64.as_deref(),
        ) {
            Ok(0) => {}
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        let mut sink: Box<dyn InstallSink> = sink_for(args.json);
        let json = args.json;
        let skip_review = args.skip_review;
        let confirm = move |plan: &crate::resolve::BuildPlan| -> crate::build::BuildDecision {
            if json {
                crate::build::BuildDecision::Review
            } else if skip_review {
                confirm_build(plan)
            } else {
                confirm_proceed_to_review(plan)
            }
        };
        let review: fn(&[crate::pkgbuild::PkgbuildInfo]) -> bool = if json {
            |_| true
        } else {
            self::review::review_pkgbuilds
        };
        let result = crate::build::run_build(
            &aur,
            false,
            args.as_deps,
            &mut *sink,
            confirm,
            review,
            args.approvals_b64.as_deref(),
        );
        if let Err(e) = &result {
            eprintln!("warning: repo packages installed; AUR phase failed: {e:#}");
        }
        exit_with_result(result);
    }
}

pub(crate) fn search_subcommand(args: SearchArgs) -> ! {
    if args.query.is_empty() {
        eprintln!("usage: pakajo search <query>");
        std::process::exit(2);
    }
    let query = args.query.join(" ");
    exit_with_result(run_search(&query));
}

pub(crate) fn aur_sync_subcommand() -> ! {
    exit_with_result(run_aur_sync());
}

pub(crate) fn gendb_subcommand() -> ! {
    exit_with_result(run_gendb());
}

pub(crate) fn clean_subcommand(args: CleanArgs) -> ! {
    exit_with_result(crate::clean::run_clean(args.remove));
}

pub(crate) fn remove_subcommand(args: RemoveArgs) -> ! {
    let positionals = dedup_positionals(args.positionals);

    if positionals.is_empty() {
        eprintln!("usage: pakajo remove [--json] <package>...");
        std::process::exit(2);
    }

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

pub(crate) fn upgrade_subcommand(args: UpgradeArgs) -> ! {
    if args.repo_only {
        let answerer = answerer_for(None);
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
        let json = args.json;
        let skip_review = args.skip_review;
        let confirm = move |plan: &crate::resolve::BuildPlan| -> crate::build::BuildDecision {
            if json {
                crate::build::BuildDecision::Review
            } else if skip_review {
                confirm_build(plan)
            } else {
                confirm_proceed_to_review(plan)
            }
        };
        let review: fn(&[crate::pkgbuild::PkgbuildInfo]) -> bool = if json {
            |_| true
        } else {
            self::review::review_pkgbuilds
        };
        let result = crate::build::run_build(
            &aur_names,
            false,
            false,
            &mut *build_sink,
            confirm,
            review,
            None,
        );
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
