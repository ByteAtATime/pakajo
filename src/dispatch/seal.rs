use anyhow::Context;

use crate::question::approvals::{SealedApprovals, validate};

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
    use crate::question::model::{Answer, Question, QuestionKey};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn fixture() -> (Vec<Question>, Vec<Answer>) {
        (
            vec![
                Question::Conflict {
                    incoming: s("foo"),
                    removable: s("bar"),
                },
                Question::InstallIgnorepkg { name: s("glibc") },
                Question::RemovePkgs {
                    names: vec![s("b"), s("a")],
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
                &Question::Proceed(crate::events::TransactionSummary {
                    packages: vec![],
                    total_download_size: 0,
                    total_installed_size: 0,
                    total_removed_size: 0,
                }),
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
