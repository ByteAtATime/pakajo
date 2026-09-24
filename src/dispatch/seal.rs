use anyhow::Context;

use crate::build::BuildDecision;
use crate::dispatch::protocol::Decider;
use crate::pkgbuild::PkgbuildInfo;
use crate::question::approvals::{SealedApprovals, validate};
use crate::question::model::{Answer, Question};
use crate::resolve::{ConflictReport, Plan};

pub struct SealedDecider {
    sealed: SealedApprovals,
}

pub(crate) const JSON_SEAL_REQUIRED: &str = "--json requires sealed approvals";

pub(crate) fn json_seal_missing(json: bool, approvals: Option<&str>) -> bool {
    json && approvals.is_none()
}

pub(crate) fn non_interactive_seal_missing(json: bool, tty: bool, approvals: Option<&str>) -> bool {
    !json && !tty && approvals.is_none()
}

pub fn sealed_decider(sealed: SealedApprovals) -> SealedDecider {
    SealedDecider { sealed }
}

pub fn proceed_decider() -> Box<dyn Decider + Send> {
    Box::new(sealed_decider(SealedApprovals {
        answers: Vec::new(),
        proceed: true,
        deps: Vec::new(),
    }))
}

impl SealedDecider {
    fn conflict_removed(&self, incoming: &str, removable: &str) -> bool {
        let key = Question::Conflict {
            incoming: incoming.to_string(),
            incoming_version: String::new(),
            removable: removable.to_string(),
            removable_version: String::new(),
            conflict_reason: None,
        }
        .key();
        let Ok(index) = self
            .sealed
            .answers
            .binary_search_by(|(stored, _)| stored.cmp(&key))
        else {
            return false;
        };
        matches!(
            &self.sealed.answers[index].1,
            Answer::Conflict { remove: true, .. }
        )
    }
}

impl Decider for SealedDecider {
    fn confirm_build(&self, _plan: &Plan) -> BuildDecision {
        BuildDecision::Proceed
    }

    fn confirm_conflicts(&self, report: &ConflictReport) -> bool {
        report
            .local
            .iter()
            .chain(report.inner.iter())
            .flat_map(|conflict| {
                conflict
                    .conflicting
                    .iter()
                    .map(|entry| (conflict.pkg.as_str(), entry.pkg.as_str()))
            })
            .all(|(incoming, removable)| self.conflict_removed(incoming, removable))
            && !report.is_empty()
    }

    fn review_pkgbuilds(&self, _pkgbuilds: &[PkgbuildInfo]) -> bool {
        true
    }
}

pub fn proceed_only_seal() -> anyhow::Result<String> {
    encode_seal(&SealedApprovals {
        answers: Vec::new(),
        proceed: true,
        deps: Vec::new(),
    })
}

pub fn encode_seal(sealed: &SealedApprovals) -> anyhow::Result<String> {
    serde_json::to_string(sealed).context("failed to encode seal")
}

pub fn decode_seal(payload: &str) -> anyhow::Result<SealedApprovals> {
    let sealed: SealedApprovals = serde_json::from_str(payload).context("failed to decode seal")?;
    validate(&sealed)?;
    Ok(sealed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::approvals::{Sealed, match_answer, seal};
    use crate::question::model::{Answer, Question, QuestionKey, TransactionKind};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn fixture() -> (Vec<Question>, Vec<Answer>) {
        (
            vec![
                Question::Conflict {
                    incoming: s("foo"),
                    incoming_version: "1.0-1".to_string(),
                    removable: s("bar"),
                    removable_version: "1.0-1".to_string(),
                    conflict_reason: None,
                },
                Question::InstallIgnorepkg { name: s("glibc") },
                Question::RemovePkgs {
                    names: vec![s("b"), s("a")],
                    kind: TransactionKind::Install,
                },
            ],
            vec![
                Answer::Conflict {
                    incoming: s("foo"),
                    removable: s("bar"),
                    remove: true,
                },
                Answer::InstallIgnorepkg {
                    name: s("glibc"),
                    install: true,
                },
                Answer::RemovePkgs {
                    names: vec![s("a"), s("b")],
                    skip: false,
                },
            ],
        )
    }

    fn encode_pairs(pairs: &[(QuestionKey, Answer)], proceed: bool) -> String {
        encode_seal(&SealedApprovals {
            answers: pairs.to_vec(),
            proceed,
            deps: Vec::new(),
        })
        .expect("encodes")
    }

    #[test]
    fn round_trip_replays_through_match_answer() {
        let (questions, answers) = fixture();
        let sealed = seal(&questions, &answers, true).expect("seal succeeds");
        let payload = encode_seal(&sealed).expect("encodes");
        let decoded = decode_seal(&payload).expect("decodes");
        assert_eq!(decoded, sealed);
        for (question, answer) in questions.iter().zip(answers.iter()) {
            assert_eq!(
                match_answer(question, &decoded),
                Sealed::Answer(answer.clone())
            );
        }
        assert_eq!(
            match_answer(
                &Question::Proceed {
                    summary: crate::events::TransactionSummary {
                        packages: vec![],
                        total_download_size: 0,
                        total_installed_size: 0,
                        total_removed_size: 0,
                    },
                    kind: TransactionKind::Install,
                },
                &decoded
            ),
            Sealed::Answer(Answer::Proceed)
        );
    }

    #[test]
    fn rejects_malformed_json_with_context() {
        let error = decode_seal("{not json").expect_err("malformed json rejected");
        assert!(error.to_string().contains("failed to decode seal"));
    }

    #[test]
    fn rejects_unsorted_answers_naming_key() {
        let (_, answers) = fixture();
        let pairs = vec![
            (
                QuestionKey::InstallIgnorepkg { name: s("glibc") },
                answers[1].clone(),
            ),
            (
                QuestionKey::Conflict {
                    first: s("bar"),
                    second: s("foo"),
                },
                answers[0].clone(),
            ),
        ];
        let error = decode_seal(&encode_pairs(&pairs, true)).expect_err("unsorted rejected");
        assert!(error.to_string().contains("not sorted"));
        assert!(error.to_string().contains("Conflict"));
    }

    #[test]
    fn rejects_duplicate_keys() {
        let (_, answers) = fixture();
        let key = QuestionKey::InstallIgnorepkg { name: s("glibc") };
        let pairs = vec![(key.clone(), answers[1].clone()), (key, answers[1].clone())];
        let error = decode_seal(&encode_pairs(&pairs, true)).expect_err("duplicate rejected");
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_non_collectable_keys() {
        let pairs = vec![(QuestionKey::Proceed, Answer::Proceed)];
        let error = decode_seal(&encode_pairs(&pairs, true)).expect_err("proceed key rejected");
        assert!(error.to_string().contains("cannot be sealed"));
        let pairs = vec![(
            QuestionKey::ImportKey {
                fingerprint: s("f"),
            },
            Answer::ImportKey {
                fingerprint: s("f"),
                import: true,
            },
        )];
        let error = decode_seal(&encode_pairs(&pairs, false)).expect_err("import key rejected");
        assert!(error.to_string().contains("cannot be sealed"));
    }

    fn conflicted_report() -> ConflictReport {
        ConflictReport {
            local: vec![crate::resolve::Conflict {
                pkg: "cava".to_string(),
                conflicting: vec![crate::resolve::Conflicting {
                    pkg: "cava-git".to_string(),
                    conflict: Some("cava".to_string()),
                }],
            }],
            ..Default::default()
        }
    }

    fn multi_conflicted_report() -> ConflictReport {
        let mut report = conflicted_report();
        report.local.push(crate::resolve::Conflict {
            pkg: "nginx".to_string(),
            conflicting: vec![crate::resolve::Conflicting {
                pkg: "nginx-mainline".to_string(),
                conflict: Some("nginx".to_string()),
            }],
        });
        report
    }

    fn conflict_seal(first: &str, second: &str, remove: bool) -> SealedApprovals {
        SealedApprovals {
            answers: vec![(
                QuestionKey::Conflict {
                    first: first.min(second).to_string(),
                    second: first.max(second).to_string(),
                },
                Answer::Conflict {
                    incoming: first.to_string(),
                    removable: second.to_string(),
                    remove,
                },
            )],
            proceed: true,
            deps: Vec::new(),
        }
    }

    #[test]
    fn sealed_decider_mirrors_automatic_semantics() {
        let decider = sealed_decider(conflict_seal("cava", "cava-git", true));
        assert_eq!(
            decider.confirm_build(&Plan {
                bases: Vec::new(),
                repo_installs: Vec::new(),
                missing: Vec::new(),
                conflicts: ConflictReport::default(),
                duplicates: Vec::new(),
            }),
            BuildDecision::Proceed
        );
        assert!(decider.review_pkgbuilds(&[]));
        assert!(!decider.confirm_conflicts(&ConflictReport::default()));
    }

    #[test]
    fn sealed_decider_proceeds_when_every_conflict_removed() {
        assert!(
            sealed_decider(conflict_seal("cava-git", "cava", true))
                .confirm_conflicts(&conflicted_report())
        );
    }

    #[test]
    fn sealed_decider_bails_when_conflict_kept() {
        assert!(
            !sealed_decider(conflict_seal("cava", "cava-git", false))
                .confirm_conflicts(&conflicted_report())
        );
    }

    #[test]
    fn sealed_decider_bails_when_a_pair_is_missing() {
        assert!(
            !sealed_decider(conflict_seal("cava", "cava-git", true))
                .confirm_conflicts(&multi_conflicted_report())
        );
    }

    #[test]
    fn sealed_decider_matches_swapped_orientation() {
        let report = conflicted_report();
        assert!(sealed_decider(conflict_seal("cava", "cava-git", true)).confirm_conflicts(&report));
        assert!(sealed_decider(conflict_seal("cava-git", "cava", true)).confirm_conflicts(&report));
    }

    #[test]
    fn rejects_non_canonical_conflict_key() {
        let pairs = vec![(
            QuestionKey::Conflict {
                first: s("foo"),
                second: s("bar"),
            },
            Answer::Conflict {
                incoming: s("bar"),
                removable: s("foo"),
                remove: true,
            },
        )];
        let error =
            decode_seal(&encode_pairs(&pairs, true)).expect_err("non-canonical key rejected");
        assert!(error.to_string().contains("does not fit"));
        assert!(error.to_string().contains("Conflict"));
    }

    #[test]
    fn rejects_mismatched_pair() {
        let pairs = vec![(
            QuestionKey::InstallIgnorepkg { name: s("glibc") },
            Answer::InstallIgnorepkg {
                name: s("zlib"),
                install: true,
            },
        )];
        let error = decode_seal(&encode_pairs(&pairs, true)).expect_err("mismatch rejected");
        assert!(error.to_string().contains("does not fit"));
    }
}
