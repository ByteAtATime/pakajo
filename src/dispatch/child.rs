use crate::cli::{ConsoleSink, EscalatedSink, JsonSink, classify_target, privs};
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

pub fn select_presentation(stream: bool, interactive: bool, tty: bool) -> Presentation {
    if !stream {
        return Presentation::Console;
    }
    if interactive && tty {
        Presentation::InteractiveStream
    } else {
        Presentation::SilentStream
    }
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

fn stdin_input() -> std::rc::Rc<std::cell::RefCell<std::io::BufReader<std::io::Stdin>>> {
    std::rc::Rc::new(std::cell::RefCell::new(std::io::BufReader::new(
        std::io::stdin(),
    )))
}

fn finish_transaction(outcome: crate::tx::driver::RunOutcome) -> anyhow::Result<i32> {
    if let crate::tx::driver::Finish::PrepareFailed(failure) = outcome.finish {
        anyhow::bail!("failed to prepare transaction: {failure}")
    }
    Ok(0)
}

fn upgrade_outcome_code(outcome: crate::tx::driver::RunOutcome) -> anyhow::Result<i32> {
    match outcome.finish {
        crate::tx::driver::Finish::Committed => Ok(0),
        crate::tx::driver::Finish::Stopped if outcome.summary.is_empty() => Ok(3),
        crate::tx::driver::Finish::Stopped => Ok(4),
        crate::tx::driver::Finish::PrepareFailed(failure) => {
            anyhow::bail!("failed to prepare transaction: {failure}")
        }
    }
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

pub fn code_from(result: anyhow::Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
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

fn upgrade_repo_source_sink(
    presentation: Presentation,
    sealed: Option<crate::question::approvals::SealedApprovals>,
) -> (
    Box<dyn crate::question::source::AnswerSource>,
    Box<dyn InstallSink>,
) {
    match presentation {
        Presentation::InteractiveStream => {
            let input = stdin_input();
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
            (source, Box::new(EscalatedSink::new()))
        }
        Presentation::Console => {
            let input = stdin_input();
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
            (source, Box::new(ConsoleSink::new()))
        }
        Presentation::SilentStream => {
            let inner: Box<dyn crate::question::source::AnswerSource> = match sealed {
                Some(sealed) => Box::new(crate::question::source::ApprovalsReplay::new(sealed)),
                None => Box::new(crate::question::source::FailClosedSource),
            };
            let source = crate::tx::prompt::stdin_channel_source(inner, |question| {
                JsonSink::new().event(InstallEvent::RuntimePrompt { question })
            });
            (source, Box::new(JsonSink::new()))
        }
    }
}

pub(crate) fn run_upgrade_repo_direct(
    no_refresh: bool,
    ignores: &[String],
    interactive: bool,
    approvals_payload: Option<&str>,
    stream: bool,
) -> anyhow::Result<crate::tx::driver::RunOutcome> {
    let presentation = select_presentation(stream, interactive, privs::stdin_is_tty());
    if !matches!(presentation, Presentation::SilentStream) {
        let (source, sink) = upgrade_repo_source_sink(presentation, None);
        return crate::upgrade::run_upgrade_repo(no_refresh, ignores, source, sink);
    }
    let sealed = approvals_payload
        .map(crate::dispatch::seal::decode_seal)
        .transpose()?;
    let (source, sink) = upgrade_repo_source_sink(Presentation::SilentStream, sealed);
    crate::upgrade::run_upgrade_repo(no_refresh, ignores, source, sink)
}

impl ChildOperation {
    pub fn execute(&self) -> anyhow::Result<i32> {
        match self {
            ChildOperation::Remove {
                targets,
                interactive,
                approvals_path,
                stream,
            } => {
                let sealed = read_seal(approvals_path.as_deref())?;
                let mut handle = crate::pacman::handle()?;
                let holds = crate::pacman::config()?.hold_pkg;
                let spec = crate::tx::driver::RunSpec {
                    kind: crate::tx::driver::RunKind::Remove(crate::tx::driver::RemoveSpec {
                        flags: alpm::TransFlag::NONE,
                        holds,
                    }),
                    targets: targets.clone(),
                    stub_targets: Vec::new(),
                    explore: false,
                    as_deps: false,
                    reinstall: false,
                    dep_names: Vec::new(),
                };
                let presentation =
                    select_presentation(*stream, *interactive, privs::stdin_is_tty());
                match presentation {
                    Presentation::InteractiveStream => {
                        let input = stdin_input();
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
                    Presentation::Console => {
                        let input = stdin_input();
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
                    }
                    Presentation::SilentStream => {
                        let inner: Box<dyn crate::question::source::AnswerSource> = match sealed {
                            Some(sealed) => {
                                Box::new(crate::question::source::ApprovalsReplay::new(sealed))
                            }
                            None => Box::new(crate::question::source::FailClosedSource),
                        };
                        let outcome = crate::tx::prompt::execute_with_source(
                            inner,
                            &mut handle,
                            &spec,
                            Box::new(JsonSink::new()),
                        )?;
                        finish_transaction(outcome)
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
                    select_presentation(*stream, *interactive, privs::stdin_is_tty());
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
                        let input = stdin_input();
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
                interactive,
                approvals_path,
                stream,
            } => {
                let presentation =
                    select_presentation(*stream, *interactive, privs::stdin_is_tty());
                let sealed = match presentation {
                    Presentation::SilentStream => read_seal(approvals_path.as_deref())?,
                    _ => None,
                };
                let (source, sink) = upgrade_repo_source_sink(presentation, sealed);
                let outcome = crate::upgrade::run_upgrade_repo(*no_refresh, ignores, source, sink)?;
                upgrade_outcome_code(outcome)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_keys_on_interactive_plus_terminal() {
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
                select_presentation(stream, interactive, tty),
                expected,
                "select_presentation(stream={stream}, interactive={interactive}, tty={tty})"
            );
        }
    }

    #[test]
    fn upgrade_direct_silent_rejects_corrupt_seal_loudly() {
        let error = run_upgrade_repo_direct(false, &[], false, Some("not json"), true)
            .expect_err("corrupt seal rejected");
        assert!(
            error.to_string().contains("failed to decode seal"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn read_seal_rejects_corrupt_file_loudly() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("pakajo-test-bad-seal-{}.json", std::process::id()));
        std::fs::write(&path, b"not json").expect("write bad seal");
        let error = read_seal(Some(&path.to_string_lossy())).expect_err("corrupt seal rejected");
        let _ = std::fs::remove_file(&path);
        assert!(
            error.to_string().contains("failed to decode seal file"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn upgrade_silent_source_replays_sealed_answer() {
        use crate::question::approvals::seal;
        use crate::question::model::{Answer, Question};
        use crate::question::source::SourceDecision;
        let question = Question::Conflict {
            incoming: "cava-git".to_string(),
            incoming_version: "1.0-1".to_string(),
            removable: "cava".to_string(),
            removable_version: "1.0-1".to_string(),
            conflict_reason: None,
        };
        let answer = Answer::Conflict {
            incoming: "cava-git".to_string(),
            removable: "cava".to_string(),
            remove: true,
        };
        let sealed = seal(std::slice::from_ref(&question), &[answer], true).expect("seal succeeds");
        let (source, _) = upgrade_repo_source_sink(Presentation::SilentStream, Some(sealed));
        assert!(
            matches!(
                source.answer(&question),
                SourceDecision::Answer(Answer::Conflict { remove: true, .. })
            ),
            "sealed conflict replays as remove"
        );
    }

    #[test]
    fn upgrade_silent_source_fails_closed_without_seal() {
        use crate::question::model::Question;
        use crate::question::source::SourceDecision;
        let question = Question::Conflict {
            incoming: "cava-git".to_string(),
            incoming_version: "1.0-1".to_string(),
            removable: "cava".to_string(),
            removable_version: "1.0-1".to_string(),
            conflict_reason: None,
        };
        let (source, _) = upgrade_repo_source_sink(Presentation::SilentStream, None);
        assert!(
            matches!(source.answer(&question), SourceDecision::Abort(_)),
            "unsealed upgrade answers nothing"
        );
    }

    #[test]
    fn read_seal_round_trips_sealed_payload() {
        use crate::question::approvals::seal;
        use crate::question::model::{Answer, Question};
        let question = Question::Conflict {
            incoming: "cava-git".to_string(),
            incoming_version: "1.0-1".to_string(),
            removable: "cava".to_string(),
            removable_version: "1.0-1".to_string(),
            conflict_reason: None,
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
    fn upgrade_outcome_codes_classify_idle_and_decline() {
        use crate::events::TransactionSummary;
        use crate::tx::driver::{Finish, RunOutcome};
        let committed = RunOutcome {
            summary: TransactionSummary::default(),
            finish: Finish::Committed,
            review: None,
        };
        assert_eq!(upgrade_outcome_code(committed).expect("committed"), 0);
        let idle = RunOutcome {
            summary: TransactionSummary::default(),
            finish: Finish::Stopped,
            review: None,
        };
        assert_eq!(upgrade_outcome_code(idle).expect("idle"), 3);
        let declined = RunOutcome {
            summary: TransactionSummary {
                packages: vec![crate::events::SummaryPackage {
                    name: "foo".to_string(),
                    repository: None,
                    new_version: "1.0".to_string(),
                    old_version: None,
                    download_size: 0,
                    installed_size: 0,
                    old_installed_size: 0,
                    is_removal: false,
                }],
                total_download_size: 0,
                total_installed_size: 0,
                total_removed_size: 0,
            },
            finish: Finish::Stopped,
            review: None,
        };
        assert_eq!(upgrade_outcome_code(declined).expect("declined"), 4);
        let failed = RunOutcome {
            summary: TransactionSummary::default(),
            finish: Finish::PrepareFailed(crate::tx::convert::PrepareFailure::Other(
                "broken".to_string(),
            )),
            review: None,
        };
        assert!(upgrade_outcome_code(failed).is_err());
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
