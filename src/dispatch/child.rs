use crate::cli::{
    ConsoleSink, EscalatedSink, JsonSink, PromptStream, answerer_for, classify_target,
    confirm_hold_remove, confirm_remove, confirm_remove_stderr, privs,
};
use crate::dispatch::operation::ChildOperation;
use crate::install::InstallTarget;
use anyhow::Context as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Console,
    InteractiveStream,
    SilentStream,
}

pub fn select_remove_presentation(stream: bool, tty: bool) -> Presentation {
    if stream {
        if tty {
            Presentation::InteractiveStream
        } else {
            Presentation::SilentStream
        }
    } else {
        Presentation::Console
    }
}

pub fn select_install_presentation(stream: bool, has_approvals: bool, tty: bool) -> Presentation {
    if !stream {
        return Presentation::Console;
    }
    if !has_approvals && tty {
        Presentation::InteractiveStream
    } else {
        Presentation::SilentStream
    }
}

pub fn select_upgrade_repo_presentation(stream: bool) -> Presentation {
    if stream {
        Presentation::SilentStream
    } else {
        Presentation::Console
    }
}

fn read_approvals(
    approvals_path: Option<&str>,
) -> anyhow::Result<Option<crate::question::Approvals>> {
    approvals_path
        .map(|path| {
            let bytes = std::fs::read(path)
                .with_context(|| format!("failed to read approvals file {path}"))?;
            serde_json::from_slice(&bytes)
                .with_context(|| format!("failed to parse approvals file {path}"))
        })
        .transpose()
}

fn finish_install(outcome: crate::tx::driver::RunOutcome) -> anyhow::Result<()> {
    if let crate::tx::driver::Finish::PrepareFailed(failure) = outcome.finish {
        anyhow::bail!("failed to prepare transaction: {failure:?}")
    }
    Ok(())
}

pub fn code_from(result: anyhow::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

pub fn run(argv: &[String]) -> i32 {
    let Some(operation) = ChildOperation::decode(argv) else {
        eprintln!("malformed dispatch argv");
        return 1;
    };
    code_from(operation.execute())
}

impl ChildOperation {
    pub fn execute(&self) -> anyhow::Result<()> {
        match self {
            ChildOperation::Remove {
                targets,
                approvals_path,
                stream,
            } => {
                let approvals = read_approvals(approvals_path.as_deref())?;
                let approved_held: Vec<String> = approvals
                    .as_ref()
                    .map(|a| a.approved_held.clone())
                    .unwrap_or_default();
                match select_remove_presentation(*stream, privs::stdin_is_tty()) {
                    Presentation::InteractiveStream => crate::remove::run_remove(
                        targets,
                        EscalatedSink::new(),
                        confirm_remove_stderr,
                        answerer_for(None),
                        &approved_held,
                        || confirm_hold_remove(PromptStream::Stderr),
                    ),
                    Presentation::SilentStream => crate::remove::run_remove(
                        targets,
                        JsonSink::new(),
                        || true,
                        answerer_for(None),
                        &approved_held,
                        || false,
                    ),
                    Presentation::Console => crate::remove::run_remove(
                        targets,
                        ConsoleSink::new(),
                        confirm_remove,
                        answerer_for(None),
                        &approved_held,
                        || confirm_hold_remove(PromptStream::Stdout),
                    ),
                }
            }
            ChildOperation::Install {
                targets,
                as_deps,
                reinstall,
                preconfirmed: _,
                approvals_path,
                stream,
            } => {
                let approvals = read_approvals(approvals_path.as_deref())?;
                let mut handle = crate::pacman::handle()?;
                let classified = targets
                    .iter()
                    .map(|s| classify_target(s))
                    .collect::<Vec<_>>();
                let repo_names: Vec<String> = classified
                    .iter()
                    .filter_map(|t| match t {
                        InstallTarget::Repo(name) => Some(name.clone()),
                        InstallTarget::File(_) => None,
                    })
                    .collect();
                if crate::tx::targets::unresolvable_target(&handle, &repo_names).is_some() {
                    anyhow::bail!(
                        "cannot build packages as root; re-run without privilege escalation"
                    );
                }
                let presentation = select_install_presentation(
                    *stream,
                    approvals.is_some(),
                    privs::stdin_is_tty(),
                );
                let answerer = answerer_for(approvals);
                match presentation {
                    Presentation::InteractiveStream | Presentation::Console => {
                        let spec = crate::tx::driver::RunSpec {
                            kind: crate::tx::driver::RunKind::Sync,
                            targets: targets.clone(),
                            explore: false,
                            as_deps: *as_deps,
                            reinstall: *reinstall,
                        };
                        let outcome = if matches!(presentation, Presentation::InteractiveStream) {
                            crate::tx::prompt::execute_with(
                                std::io::BufReader::new(std::io::stdin()),
                                std::io::stderr(),
                                &mut handle,
                                &spec,
                            )?
                        } else {
                            crate::tx::prompt::execute(&mut handle, &spec)?
                        };
                        finish_install(outcome)
                    }
                    Presentation::SilentStream => crate::install::run_install(
                        &classified,
                        *as_deps,
                        JsonSink::new(),
                        || true,
                        answerer,
                    ),
                }
            }
            ChildOperation::UpgradeRepo {
                no_refresh,
                ignores,
                fingerprint_path,
                approvals_path,
                stream,
            } => {
                let approvals = read_approvals(approvals_path.as_deref())?;
                let answerer = answerer_for(approvals);
                let silent = matches!(
                    select_upgrade_repo_presentation(*stream),
                    Presentation::SilentStream
                );
                if silent {
                    crate::upgrade::run_repo_sysupgrade(
                        *no_refresh,
                        ignores,
                        JsonSink::new(),
                        answerer,
                        fingerprint_path.as_deref(),
                    )
                } else {
                    crate::upgrade::run_repo_sysupgrade(
                        *no_refresh,
                        ignores,
                        ConsoleSink::new(),
                        answerer,
                        fingerprint_path.as_deref(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_presentation_keys_on_terminal_alone() {
        use Presentation::{Console, InteractiveStream, SilentStream};
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
        use Presentation::{Console, InteractiveStream, SilentStream};
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
        use Presentation::{Console, SilentStream};
        let cases = [(true, SilentStream), (false, Console)];
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
        let result = read_approvals(Some(&path.to_string_lossy()));
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }
}
