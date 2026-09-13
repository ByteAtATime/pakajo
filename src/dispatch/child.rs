use crate::cli::{ConsoleSink, classify_target, privs};
use crate::cli::{EscalatedSink, JsonSink, answerer_for, confirm_install, confirm_install_stderr};
use crate::cli::{confirm_remove, confirm_remove_stderr};
use crate::dispatch::operation::ChildOperation;
use crate::install::InstallTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovePresentation {
    InteractiveStream,
    SilentStream,
    Console,
}

pub fn select_remove_presentation(stream: bool, tty: bool) -> RemovePresentation {
    if stream {
        if tty {
            RemovePresentation::InteractiveStream
        } else {
            RemovePresentation::SilentStream
        }
    } else {
        RemovePresentation::Console
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallPresentation {
    InteractiveStream,
    SilentStream,
    Console,
}

pub fn select_install_presentation(
    stream: bool,
    has_approvals: bool,
    tty: bool,
) -> InstallPresentation {
    if !stream {
        return InstallPresentation::Console;
    }
    if !has_approvals && tty {
        InstallPresentation::InteractiveStream
    } else {
        InstallPresentation::SilentStream
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeRepoPresentation {
    Stream,
    Console,
}

pub fn select_upgrade_repo_presentation(stream: bool) -> UpgradeRepoPresentation {
    if stream {
        UpgradeRepoPresentation::Stream
    } else {
        UpgradeRepoPresentation::Console
    }
}

pub fn read_approvals_file(path: &str) -> anyhow::Result<crate::question::Approvals> {
    let bytes = std::fs::read(path).map_err(|_| anyhow::anyhow!("malformed dispatch argv"))?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("malformed dispatch argv"))
}

pub fn run(argv: &[String]) -> ! {
    match ChildOperation::decode(argv) {
        Some(ChildOperation::Remove { targets, stream }) => run_remove_root(&targets, stream),
        Some(ChildOperation::Install {
            targets,
            as_deps,
            approvals_path,
            stream,
        }) => {
            let approvals = approvals_path
                .as_deref()
                .map(read_approvals_file)
                .transpose();
            match approvals {
                Ok(approvals) => run_install_root(&targets, as_deps, stream, approvals),
                Err(e) => {
                    eprintln!("{e:#}");
                    std::process::exit(1);
                }
            }
        }
        Some(ChildOperation::UpgradeRepo {
            no_refresh,
            ignores,
            fingerprint_path,
            approvals_path,
            stream,
        }) => {
            let approvals = approvals_path
                .as_deref()
                .map(read_approvals_file)
                .transpose();
            match approvals {
                Ok(approvals) => run_upgrade_repo_root(
                    no_refresh,
                    &ignores,
                    fingerprint_path.as_deref(),
                    stream,
                    approvals,
                ),
                Err(e) => {
                    eprintln!("{e:#}");
                    std::process::exit(1);
                }
            }
        }
        None => {
            eprintln!("malformed dispatch argv");
            std::process::exit(1);
        }
    }
}

pub fn run_remove_root(targets: &[String], stream: bool) -> ! {
    match select_remove_presentation(stream, privs::stdin_is_tty()) {
        RemovePresentation::InteractiveStream => exit_with_result(crate::remove::run_remove(
            targets,
            EscalatedSink::new(),
            confirm_remove_stderr,
            answerer_for(None),
        )),
        RemovePresentation::SilentStream => exit_with_result(crate::remove::run_remove(
            targets,
            JsonSink::new(),
            || true,
            answerer_for(None),
        )),
        RemovePresentation::Console => exit_with_result(crate::remove::run_remove(
            targets,
            ConsoleSink::new(),
            confirm_remove,
            answerer_for(None),
        )),
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

pub fn run_upgrade_repo_root(
    no_refresh: bool,
    ignores: &[String],
    fingerprint_path: Option<&str>,
    stream: bool,
    approvals: Option<crate::question::Approvals>,
) -> ! {
    let answerer = answerer_for(approvals);
    match select_upgrade_repo_presentation(stream) {
        UpgradeRepoPresentation::Stream => exit_with_result(crate::upgrade::run_repo_sysupgrade(
            no_refresh,
            ignores,
            JsonSink::new(),
            answerer,
            fingerprint_path,
        )),
        UpgradeRepoPresentation::Console => exit_with_result(crate::upgrade::run_repo_sysupgrade(
            no_refresh,
            ignores,
            ConsoleSink::new(),
            answerer,
            fingerprint_path,
        )),
    }
}

pub fn run_install_root(
    positionals: &[String],
    as_deps: bool,
    stream: bool,
    approvals: Option<crate::question::Approvals>,
) -> ! {
    let handle = match crate::cli::alpm_handle() {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    let targets = positionals
        .iter()
        .map(|s| classify_target(s))
        .collect::<Vec<_>>();
    let needs_lookup = targets.iter().any(|t| matches!(t, InstallTarget::Repo(_)));
    if needs_lookup {
        for target in &targets {
            if let InstallTarget::Repo(name) = target
                && !crate::package::repo_exists(&handle, name)
            {
                eprintln!("cannot build packages as root; re-run without privilege escalation");
                std::process::exit(1);
            }
        }
    }
    let presentation =
        select_install_presentation(stream, approvals.is_some(), privs::stdin_is_tty());
    let answerer = answerer_for(approvals);
    match presentation {
        InstallPresentation::InteractiveStream => {
            exit_with_result(crate::install::run_install(
                &targets,
                as_deps,
                EscalatedSink::new(),
                confirm_install_stderr,
                answerer,
            ));
        }
        InstallPresentation::SilentStream => exit_with_result(crate::install::run_install(
            &targets,
            as_deps,
            JsonSink::new(),
            || true,
            answerer,
        )),
        InstallPresentation::Console => exit_with_result(crate::install::run_install(
            &targets,
            as_deps,
            ConsoleSink::new(),
            confirm_install,
            answerer,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_presentation_keys_on_terminal_alone() {
        use RemovePresentation::{Console, InteractiveStream, SilentStream};
        let cases = [
            ((true, true), InteractiveStream),
            ((true, false), SilentStream),
            ((false, true), Console),
            ((false, false), Console),
        ];
        for ((stream, tty), expected) in cases {
            assert_eq!(
                select_remove_presentation(stream, tty),
                expected,
                "select_remove_presentation(stream={stream}, tty={tty})"
            );
        }
    }

    #[test]
    fn install_presentation_requires_no_approvals_plus_terminal() {
        use InstallPresentation::{Console, InteractiveStream, SilentStream};
        let cases = [
            ((true, false, true), InteractiveStream),
            ((true, true, true), SilentStream),
            ((true, false, false), SilentStream),
            ((true, true, false), SilentStream),
            ((false, false, true), Console),
            ((false, true, true), Console),
            ((false, false, false), Console),
            ((false, true, false), Console),
        ];
        for ((stream, has_approvals, tty), expected) in cases {
            assert_eq!(
                select_install_presentation(stream, has_approvals, tty),
                expected,
                "select_install_presentation(stream={stream}, has_approvals={has_approvals}, tty={tty})"
            );
        }
    }

    #[test]
    fn upgrade_repo_presentation_keys_on_stream_alone() {
        use UpgradeRepoPresentation::{Console, Stream};
        let cases = [(true, Stream), (false, Console)];
        for (stream, expected) in cases {
            assert_eq!(
                select_upgrade_repo_presentation(stream),
                expected,
                "select_upgrade_repo_presentation(stream={stream})"
            );
        }
    }

    #[test]
    fn approvals_file_rejects_malformed_json() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("pakajo-test-bad-{}.json", std::process::id()));
        std::fs::write(&path, b"not json").expect("write bad approvals");
        let result = read_approvals_file(&path.to_string_lossy());
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }
}
