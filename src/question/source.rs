use serde::{Deserialize, Serialize};

use super::approvals::{Sealed, SealedApprovals, match_answer};
use super::model::{Answer, Question, QuestionKey};

pub trait AnswerSource {
    fn answer(&self, question: &Question) -> SourceDecision;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDecision {
    Answer(Answer),
    Abort(FailClosed),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailClosed {
    pub key: QuestionKey,
    pub reason: String,
}

impl std::fmt::Display for FailClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "aborted: {:?}: {}", self.key, self.reason)
    }
}

pub struct ExploreDefaults;

const RUNTIME_NOT_PROMPTABLE: &str =
    "explore cannot answer runtime questions; they require a prompting source";

impl AnswerSource for ExploreDefaults {
    fn answer(&self, question: &Question) -> SourceDecision {
        match question {
            Question::SelectProvider { candidates, .. } => match candidates.first() {
                Some(first) => SourceDecision::Answer(Answer::SelectProvider {
                    name: first.name.clone(),
                    repo: first.repo.clone(),
                }),
                None => SourceDecision::Abort(FailClosed {
                    key: question.key(),
                    reason: "no provider candidates were offered".to_string(),
                }),
            },
            Question::Conflict {
                incoming,
                removable,
            } => SourceDecision::Answer(Answer::Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
                remove: true,
            }),
            Question::Replace { old, new, .. } => SourceDecision::Answer(Answer::Replace {
                old: old.clone(),
                new: new.clone(),
                replace: true,
            }),
            Question::InstallIgnorepkg { name } => {
                SourceDecision::Answer(Answer::InstallIgnorepkg {
                    name: name.clone(),
                    install: false,
                })
            }
            Question::RemovePkgs { names, .. } => SourceDecision::Answer(Answer::RemovePkgs {
                names: names.clone(),
                skip: true,
            }),
            Question::HoldPkgs { names } => SourceDecision::Answer(Answer::HoldPkgs {
                names: names.clone(),
                proceed: true,
            }),
            Question::GroupMembers { members, .. } => {
                SourceDecision::Answer(Answer::GroupMembers {
                    selected: members.clone(),
                })
            }
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Stop),
            Question::Corrupted { .. } | Question::ImportKey { .. } => {
                SourceDecision::Abort(FailClosed {
                    key: question.key(),
                    reason: RUNTIME_NOT_PROMPTABLE.to_string(),
                })
            }
        }
    }
}

pub struct FailClosedSource;

const NO_ANSWER: &str = "fail-closed source has no answer";

impl AnswerSource for FailClosedSource {
    fn answer(&self, question: &Question) -> SourceDecision {
        SourceDecision::Abort(FailClosed {
            key: question.key(),
            reason: NO_ANSWER.to_string(),
        })
    }
}

pub struct ApprovalsReplay {
    sealed: SealedApprovals,
}

impl ApprovalsReplay {
    pub fn new(sealed: SealedApprovals) -> Self {
        Self { sealed }
    }
}

impl AnswerSource for ApprovalsReplay {
    fn answer(&self, question: &Question) -> SourceDecision {
        match match_answer(question, &self.sealed) {
            Sealed::Answer(answer) => SourceDecision::Answer(answer),
            Sealed::Ask if matches!(question, Question::Proceed { .. }) => {
                SourceDecision::Answer(Answer::Stop)
            }
            Sealed::Ask => SourceDecision::Abort(FailClosed {
                key: question.key(),
                reason: "unanswered question".to_string(),
            }),
        }
    }
}

pub trait RuntimePrompter {
    fn import_key(&self, fingerprint: &str, uid: &str) -> bool;
}

pub struct RuntimeSource<P: RuntimePrompter> {
    inner: Box<dyn AnswerSource>,
    prompter: P,
}

impl<P: RuntimePrompter> RuntimeSource<P> {
    pub fn new(inner: Box<dyn AnswerSource>, prompter: P) -> Self {
        Self { inner, prompter }
    }
}

impl<P: RuntimePrompter> AnswerSource for RuntimeSource<P> {
    fn answer(&self, question: &Question) -> SourceDecision {
        match question {
            Question::ImportKey { fingerprint, uid } => SourceDecision::Answer(Answer::ImportKey {
                fingerprint: fingerprint.clone(),
                import: self.prompter.import_key(fingerprint, uid),
            }),
            Question::Corrupted { path } => match self.inner.answer(question) {
                SourceDecision::Abort(_) => SourceDecision::Answer(Answer::Corrupted {
                    path: path.clone(),
                    remove: true,
                }),
                decision => decision,
            },
            question => self.inner.answer(question),
        }
    }
}

pub fn parse_provider_selection(input: &str, candidate_count: usize) -> Option<usize> {
    if candidate_count == 0 {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(1);
    }
    parse_member_number(trimmed, candidate_count)
}

pub fn parse_group_selection(input: &str, member_count: usize) -> Option<Vec<usize>> {
    if input.trim().is_empty() {
        return Some((1..=member_count).collect());
    }
    let mut selected = Vec::new();
    for token in input.split_whitespace() {
        for n in parse_group_token(token, member_count)? {
            if !selected.contains(&n) {
                selected.push(n);
            }
        }
    }
    Some(selected)
}

fn parse_group_token(token: &str, member_count: usize) -> Option<Vec<usize>> {
    let Some((start, end)) = token.split_once('-') else {
        return parse_member_number(token, member_count).map(|n| vec![n]);
    };
    let lo = parse_member_number(start, member_count)?;
    let hi = parse_member_number(end, member_count)?;
    if lo > hi {
        return None;
    }
    Some((lo..=hi).collect())
}

fn parse_member_number(text: &str, member_count: usize) -> Option<usize> {
    match text.parse::<usize>() {
        Ok(n) if (1..=member_count).contains(&n) => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::approvals::seal;
    use super::super::model::ProviderCandidate;
    use super::super::model::TransactionKind;
    use super::*;

    fn provider(name: &str, repo: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: name.to_string(),
            repo: Some(repo.to_string()),
            version: None,
        }
    }

    fn summary() -> crate::events::TransactionSummary {
        crate::events::TransactionSummary::default()
    }

    fn s(value: &str) -> String {
        value.to_string()
    }

    #[test]
    fn explore_defaults_follow_matrix_and_fail_closed() {
        let explore = ExploreDefaults;
        let cases: Vec<(Question, Answer)> = vec![
            (
                Question::SelectProvider {
                    depend: "virt".to_string(),
                    candidates: vec![
                        provider("provider-one", "core"),
                        provider("provider-two", "extra"),
                    ],
                },
                Answer::SelectProvider {
                    name: "provider-one".to_string(),
                    repo: Some("core".to_string()),
                },
            ),
            (
                Question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                },
                Answer::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                    remove: true,
                },
            ),
            (
                Question::Replace {
                    old: "nginx".to_string(),
                    new: "nginx-mainline".to_string(),
                    repo: Some("extra".to_string()),
                },
                Answer::Replace {
                    old: "nginx".to_string(),
                    new: "nginx-mainline".to_string(),
                    replace: true,
                },
            ),
            (
                Question::InstallIgnorepkg {
                    name: "glibc".to_string(),
                },
                Answer::InstallIgnorepkg {
                    name: "glibc".to_string(),
                    install: false,
                },
            ),
            (
                Question::RemovePkgs {
                    names: vec!["gone".to_string(), "also-gone".to_string()],
                    kind: TransactionKind::Install,
                },
                Answer::RemovePkgs {
                    names: vec!["gone".to_string(), "also-gone".to_string()],
                    skip: true,
                },
            ),
            (
                Question::HoldPkgs {
                    names: vec!["sl".to_string()],
                },
                Answer::HoldPkgs {
                    names: vec!["sl".to_string()],
                    proceed: true,
                },
            ),
            (
                Question::GroupMembers {
                    group: "tools".to_string(),
                    members: vec!["a".to_string(), "b".to_string()],
                },
                Answer::GroupMembers {
                    selected: vec!["a".to_string(), "b".to_string()],
                },
            ),
        ];
        for (question, expected) in cases {
            assert_eq!(explore.answer(&question), SourceDecision::Answer(expected));
        }
        assert_eq!(
            explore.answer(&Question::Proceed {
                summary: summary(),
                kind: TransactionKind::Install,
            }),
            SourceDecision::Answer(Answer::Stop)
        );
        for question in [
            Question::Corrupted {
                path: "/cache/foo.pkg.tar.zst".to_string(),
            },
            Question::ImportKey {
                fingerprint: "ABC".to_string(),
                uid: "root".to_string(),
            },
        ] {
            let SourceDecision::Abort(denied) = explore.answer(&question) else {
                panic!("expected fail-closed for {question:?}");
            };
            assert_eq!(denied.key, question.key());
            assert!(denied.reason.contains("runtime"), "{}", denied.reason);
        }
        let SourceDecision::Abort(denied) = explore.answer(&Question::SelectProvider {
            depend: "virt".to_string(),
            candidates: vec![],
        }) else {
            panic!("expected fail-closed for empty provider candidates");
        };
        assert_eq!(
            denied.key,
            QuestionKey::SelectProvider {
                depend: "virt".to_string()
            }
        );
    }

    #[test]
    fn selection_parsing() {
        for (input, count, expected) in [
            ("", 3, Some(1)),
            ("2", 3, Some(2)),
            ("3", 3, Some(3)),
            ("x", 3, None),
            ("0", 3, None),
            ("4", 3, None),
            ("", 0, None),
        ] {
            assert_eq!(parse_provider_selection(input, count), expected);
        }
        for (input, count, expected) in [
            ("", 5, Some(vec![1, 2, 3, 4, 5])),
            ("3", 5, Some(vec![3])),
            ("1-3 5", 5, Some(vec![1, 2, 3, 5])),
            ("1-3 2", 5, Some(vec![1, 2, 3])),
            ("3-1", 5, None),
            ("9", 5, None),
            ("1-x", 5, None),
            ("2 9", 5, None),
            ("", 0, Some(vec![])),
        ] {
            assert_eq!(parse_group_selection(input, count), expected);
        }
    }

    #[test]
    fn fail_closed_source_aborts_with_key() {
        let source = FailClosedSource;
        for question in [
            Question::Conflict {
                incoming: s("foo"),
                removable: s("bar"),
            },
            Question::Proceed {
                summary: summary(),
                kind: TransactionKind::Install,
            },
        ] {
            assert_eq!(
                source.answer(&question),
                SourceDecision::Abort(FailClosed {
                    key: question.key(),
                    reason: NO_ANSWER.to_string(),
                })
            );
        }
    }

    #[test]
    fn approvals_replay_answers_aborts_and_maps_proceed() {
        let conflict = Question::Conflict {
            incoming: s("foo"),
            removable: s("bar"),
        };
        let answer = Answer::Conflict {
            incoming: s("foo"),
            removable: s("bar"),
            remove: true,
        };
        let replay = ApprovalsReplay::new(
            seal(
                std::slice::from_ref(&conflict),
                std::slice::from_ref(&answer),
                true,
            )
            .expect("seal succeeds"),
        );
        assert_eq!(replay.answer(&conflict), SourceDecision::Answer(answer));
        let unsealed = Question::SelectProvider {
            depend: s("sdl"),
            candidates: vec![],
        };
        assert_eq!(
            replay.answer(&unsealed),
            SourceDecision::Abort(FailClosed {
                key: unsealed.key(),
                reason: "unanswered question".to_string(),
            })
        );
        assert_eq!(
            replay.answer(&Question::Proceed {
                summary: summary(),
                kind: TransactionKind::Install,
            }),
            SourceDecision::Answer(Answer::Proceed)
        );
        let declined = ApprovalsReplay::new(seal(&[], &[], false).expect("seal succeeds"));
        assert_eq!(
            declined.answer(&Question::Proceed {
                summary: summary(),
                kind: TransactionKind::Install,
            }),
            SourceDecision::Answer(Answer::Stop)
        );
    }

    struct KeepCorrupted;

    impl AnswerSource for KeepCorrupted {
        fn answer(&self, question: &Question) -> SourceDecision {
            let Question::Corrupted { path } = question else {
                return SourceDecision::Abort(FailClosed {
                    key: question.key(),
                    reason: s("unexpected"),
                });
            };
            SourceDecision::Answer(Answer::Corrupted {
                path: path.clone(),
                remove: false,
            })
        }
    }

    struct FakePrompter {
        verdict: bool,
    }

    impl RuntimePrompter for FakePrompter {
        fn import_key(&self, _fingerprint: &str, _uid: &str) -> bool {
            self.verdict
        }
    }

    fn runtime_over(inner: Box<dyn AnswerSource>, verdict: bool) -> RuntimeSource<FakePrompter> {
        RuntimeSource::new(inner, FakePrompter { verdict })
    }

    #[test]
    fn runtime_source_handles_runtime_questions_and_delegates_rest() {
        let import = Question::ImportKey {
            fingerprint: s("ABC"),
            uid: s("root"),
        };
        for (verdict, expected) in [(true, true), (false, false)] {
            assert_eq!(
                runtime_over(Box::new(FailClosedSource), verdict).answer(&import),
                SourceDecision::Answer(Answer::ImportKey {
                    fingerprint: s("ABC"),
                    import: expected,
                })
            );
        }
        let corrupted = Question::Corrupted {
            path: s("/cache/p.pkg.tar.zst"),
        };
        assert_eq!(
            runtime_over(Box::new(KeepCorrupted), true).answer(&corrupted),
            SourceDecision::Answer(Answer::Corrupted {
                path: s("/cache/p.pkg.tar.zst"),
                remove: false,
            })
        );
        assert_eq!(
            runtime_over(Box::new(FailClosedSource), true).answer(&corrupted),
            SourceDecision::Answer(Answer::Corrupted {
                path: s("/cache/p.pkg.tar.zst"),
                remove: true,
            })
        );
        let proceed = Question::Proceed {
            summary: summary(),
            kind: TransactionKind::Install,
        };
        assert_eq!(
            runtime_over(Box::new(FailClosedSource), true).answer(&proceed),
            SourceDecision::Abort(FailClosed {
                key: QuestionKey::Proceed,
                reason: NO_ANSWER.to_string(),
            })
        );
    }
}
