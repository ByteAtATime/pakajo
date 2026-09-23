use crate::cli::{
    ConsoleSink, EscalatedSink, JsonSink, PromptStream, answerer_for, classify_target,
    confirm_hold_remove, confirm_remove, privs,
};
use crate::dispatch::operation::ChildOperation;
use crate::events::{InstallEvent, InstallSink};
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

pub fn select_install_presentation(stream: bool, interactive: bool, tty: bool) -> Presentation {
    if !stream {
        return Presentation::Console;
    }
    if interactive && tty {
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

fn read_seal(
    approvals_path: Option<&str>,
) -> anyhow::Result<Option<crate::question::approvals::SealedApprovals>> {
    approvals_path
        .map(|path| {
            let payload = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read seal file {path}"))?;
            crate::dispatch::seal::decode_seal(&payload)
                .with_context(|| format!("failed to decode seal file {path}"))
        })
        .transpose()
}

fn finish_transaction(outcome: crate::tx::driver::RunOutcome) -> anyhow::Result<()> {
    if let crate::tx::driver::Finish::PrepareFailed(failure) = outcome.finish {
        anyhow::bail!("failed to prepare transaction: {failure}")
    }
    Ok(())
}

fn engine_source<R: std::io::BufRead + 'static, W: std::io::Write + 'static>(
    preconfirmed: bool,
    source: crate::tx::prompt::InteractiveSource<R, W>,
    prompter: crate::tx::prompt::TtyImportPrompter<R, W>,
) -> Box<dyn crate::question::source::AnswerSource> {
    let runtime = crate::tx::prompt::tty_runtime_source(source, prompter);
    if preconfirmed {
        crate::tx::prompt::with_preapproved_proceed(runtime)
    } else {
        runtime
    }
}

fn repo_names_acceptable(handle: &alpm::Alpm, repo_names: &[String]) -> bool {
    let probe: Vec<String> = repo_names
        .iter()
        .filter(|name| crate::package::find_groups(handle, name).is_empty())
        .cloned()
        .collect();
    crate::tx::targets::unresolvable_target(handle, &probe).is_none()
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
                interactive,
                approvals_path,
                stream,
            } => {
                let approvals = read_approvals(approvals_path.as_deref())?;
                let approved_held: Vec<String> = approvals
                    .as_ref()
                    .map(|a| a.approved_held.clone())
                    .unwrap_or_default();
                match select_remove_presentation(*stream, privs::stdin_is_tty()) {
                    Presentation::InteractiveStream => {
                        let mut handle = crate::pacman::handle()?;
                        let holds = crate::pacman::config()?.hold_pkg;
                        let spec = crate::tx::driver::RunSpec {
                            kind: crate::tx::driver::RunKind::Remove(
                                crate::tx::driver::RemoveSpec {
                                    flags: alpm::TransFlag::NONE,
                                    holds,
                                },
                            ),
                            targets: targets.clone(),
                            stub_targets: Vec::new(),
                            explore: false,
                            as_deps: false,
                            reinstall: false,
                            dep_names: Vec::new(),
                        };
                        let input = std::rc::Rc::new(std::cell::RefCell::new(
                            std::io::BufReader::new(std::io::stdin()),
                        ));
                        let source = engine_source(
                            false,
                            crate::tx::prompt::InteractiveSource::new(
                                std::rc::Rc::clone(&input),
                                std::io::stderr(),
                                crate::color::stderr_color(),
                            ),
                            crate::tx::prompt::TtyImportPrompter::new(
                                std::rc::Rc::clone(&input),
                                std::io::stderr(),
                                crate::color::stderr_color(),
                            ),
                        );
                        let outcome = crate::tx::prompt::execute_with_source(
                            source,
                            &mut handle,
                            &spec,
                            Box::new(EscalatedSink::new()),
                        )?;
                        finish_transaction(outcome)
                    }
                    Presentation::SilentStream => crate::remove::run_remove(
                        targets,
                        JsonSink::new(),
                        || true,
                        answerer_for(None),
                        &approved_held,
                        || false,
                    ),
                    Presentation::Console => {
                        if *interactive {
                            let mut handle = crate::pacman::handle()?;
                            let holds = crate::pacman::config()?.hold_pkg;
                            let spec = crate::tx::driver::RunSpec {
                                kind: crate::tx::driver::RunKind::Remove(
                                    crate::tx::driver::RemoveSpec {
                                        flags: alpm::TransFlag::NONE,
                                        holds,
                                    },
                                ),
                                targets: targets.clone(),
                                stub_targets: Vec::new(),
                                explore: false,
                                as_deps: false,
                                reinstall: false,
                                dep_names: Vec::new(),
                            };
                            let input = std::rc::Rc::new(std::cell::RefCell::new(
                                std::io::BufReader::new(std::io::stdin()),
                            ));
                            let source = engine_source(
                                false,
                                crate::tx::prompt::InteractiveSource::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stdout(),
                                    crate::color::stdout_color(),
                                ),
                                crate::tx::prompt::TtyImportPrompter::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stdout(),
                                    crate::color::stdout_color(),
                                ),
                            );
                            let outcome = crate::tx::prompt::execute_with_source(
                                source,
                                &mut handle,
                                &spec,
                                Box::new(ConsoleSink::new()),
                            )?;
                            finish_transaction(outcome)
                        } else {
                            crate::remove::run_remove(
                                targets,
                                ConsoleSink::new(),
                                confirm_remove,
                                answerer_for(None),
                                &approved_held,
                                || confirm_hold_remove(PromptStream::Stdout),
                            )
                        }
                    }
                }
            }
            ChildOperation::Install {
                targets,
                as_deps,
                reinstall,
                preconfirmed,
                interactive,
                approvals_path,
                stream,
            } => {
                let sealed = read_seal(approvals_path.as_deref())?;
                let dep_names = sealed
                    .as_ref()
                    .map(|sealed| sealed.deps.clone())
                    .unwrap_or_default();
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
                if !repo_names_acceptable(&handle, &repo_names) {
                    anyhow::bail!(
                        "cannot build packages as root; re-run without privilege escalation"
                    );
                }
                let presentation =
                    select_install_presentation(*stream, *interactive, privs::stdin_is_tty());
                let preconfirmed = *preconfirmed;
                match presentation {
                    Presentation::InteractiveStream | Presentation::Console => {
                        let spec = crate::tx::driver::RunSpec {
                            kind: crate::tx::driver::RunKind::Sync,
                            targets: targets.clone(),
                            stub_targets: Vec::new(),
                            explore: false,
                            as_deps: *as_deps,
                            reinstall: *reinstall,
                            dep_names: dep_names.clone(),
                        };
                        let input = std::rc::Rc::new(std::cell::RefCell::new(
                            std::io::BufReader::new(std::io::stdin()),
                        ));
                        let outcome = if matches!(presentation, Presentation::InteractiveStream) {
                            let source = engine_source(
                                preconfirmed,
                                crate::tx::prompt::InteractiveSource::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stderr(),
                                    crate::color::stderr_color(),
                                ),
                                crate::tx::prompt::TtyImportPrompter::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stderr(),
                                    crate::color::stderr_color(),
                                ),
                            );
                            crate::tx::prompt::execute_with_source(
                                source,
                                &mut handle,
                                &spec,
                                Box::new(EscalatedSink::new()),
                            )?
                        } else {
                            let source = engine_source(
                                preconfirmed,
                                crate::tx::prompt::InteractiveSource::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stdout(),
                                    crate::color::stdout_color(),
                                ),
                                crate::tx::prompt::TtyImportPrompter::new(
                                    std::rc::Rc::clone(&input),
                                    std::io::stdout(),
                                    crate::color::stdout_color(),
                                ),
                            );
                            crate::tx::prompt::execute_with_source(
                                source,
                                &mut handle,
                                &spec,
                                Box::new(ConsoleSink::new()),
                            )?
                        };
                        finish_transaction(outcome)
                    }
                    Presentation::SilentStream => {
                        let spec = crate::tx::driver::RunSpec {
                            kind: crate::tx::driver::RunKind::Sync,
                            targets: targets.clone(),
                            stub_targets: Vec::new(),
                            explore: false,
                            as_deps: *as_deps,
                            reinstall: *reinstall,
                            dep_names,
                        };
                        let inner: Box<dyn crate::question::source::AnswerSource> = match sealed {
                            Some(sealed) => {
                                Box::new(crate::question::source::ApprovalsReplay::new(sealed))
                            }
                            None => Box::new(crate::question::source::FailClosedSource),
                        };
                        let source = crate::tx::prompt::stdin_channel_source(inner, |question| {
                            JsonSink::new().event(InstallEvent::RuntimePrompt { question })
                        });
                        let outcome = crate::tx::prompt::execute_with_source(
                            source,
                            &mut handle,
                            &spec,
                            Box::new(JsonSink::new()),
                        )?;
                        finish_transaction(outcome)
                    }
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
    fn install_presentation_keys_on_interactive_plus_terminal() {
        use Presentation::{Console, InteractiveStream, SilentStream};
        let cases = [
            ((true, true, true), InteractiveStream),
            ((true, false, true), SilentStream),
            ((true, true, false), SilentStream),
            ((true, false, false), SilentStream),
            ((false, false, true), Console),
            ((false, true, true), Console),
            ((false, false, false), Console),
            ((false, true, false), Console),
        ];
        for ((stream, interactive, tty), expected) in cases {
            assert_eq!(
                select_install_presentation(stream, interactive, tty),
                expected,
                "select_install_presentation(stream={stream}, interactive={interactive}, tty={tty})"
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

    #[test]
    fn read_seal_round_trips_sealed_payload() {
        use crate::question::approvals::seal;
        use crate::question::model::{Answer, Question};
        let question = Question::Conflict {
            incoming: "cava-git".to_string(),
            removable: "cava".to_string(),
        };
        let answer = Answer::Conflict {
            incoming: "cava-git".to_string(),
            removable: "cava".to_string(),
            remove: true,
        };
        let sealed = seal(std::slice::from_ref(&question), &[answer], true).expect("seal succeeds");
        let payload = crate::dispatch::seal::encode_seal(&sealed).expect("encodes");
        let dir = std::env::temp_dir();
        let path = dir.join(format!("pakajo-test-seal-{}.json", std::process::id()));
        std::fs::write(&path, payload).expect("write seal");
        let decoded = read_seal(Some(&path.to_string_lossy())).expect("reads seal");
        let _ = std::fs::remove_file(&path);
        assert_eq!(decoded, Some(sealed));
        assert!(read_seal(None).expect("no path is no seal").is_none());
    }

    #[test]
    fn finish_transaction_maps_outcomes() {
        use crate::tx::driver::{Finish, RunOutcome};
        for finish in [Finish::Committed, Finish::Stopped] {
            let outcome = RunOutcome {
                summary: crate::events::TransactionSummary::default(),
                finish,
                review: None,
            };
            assert!(finish_transaction(outcome).is_ok());
        }
        let outcome = RunOutcome {
            summary: crate::events::TransactionSummary::default(),
            finish: Finish::PrepareFailed(crate::tx::convert::PrepareFailure::Other(
                "broken".to_string(),
            )),
            review: None,
        };
        let error = finish_transaction(outcome).expect_err("prepare failure errors");
        assert!(
            error
                .to_string()
                .contains("failed to prepare transaction: broken")
        );
    }
}
