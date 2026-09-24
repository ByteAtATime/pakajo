use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::{Answer, Question, QuestionKey};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedApprovals {
    pub answers: Vec<(QuestionKey, Answer)>,
    pub proceed: bool,
    #[serde(default)]
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sealed {
    Answer(Answer),
    Ask,
}

pub fn seal(
    questions: &[Question],
    answers: &[Answer],
    proceed: bool,
) -> anyhow::Result<SealedApprovals> {
    if questions.len() != answers.len() {
        anyhow::bail!(
            "answer count {} does not match question count {}",
            answers.len(),
            questions.len()
        );
    }
    let mut keyed = BTreeMap::new();
    for (question, answer) in questions.iter().zip(answers.iter()) {
        validate_pair(question, answer)?;
        let key = question.key();
        if let Some(seen) = keyed.insert(key.clone(), answer.clone())
            && seen != *answer
        {
            anyhow::bail!("question {:?} was sealed with divergent answers", key);
        }
    }
    Ok(SealedApprovals {
        answers: keyed.into_iter().collect(),
        proceed,
        deps: Vec::new(),
    })
}

pub fn match_answer(question: &Question, sealed: &SealedApprovals) -> Sealed {
    match question {
        Question::Proceed { .. } if sealed.proceed => Sealed::Answer(Answer::Proceed),
        Question::Proceed { .. } => Sealed::Ask,
        question if question.is_runtime() => Sealed::Ask,
        question => replay_answer(question, sealed),
    }
}

fn replay_answer(question: &Question, sealed: &SealedApprovals) -> Sealed {
    let key = question.key();
    let Ok(index) = sealed
        .answers
        .binary_search_by(|(stored, _)| stored.cmp(&key))
    else {
        return Sealed::Ask;
    };
    let stored = &sealed.answers[index].1;
    if !answer_fits_question(stored, question) {
        return Sealed::Ask;
    }
    Sealed::Answer(stored.clone())
}

fn validate_pair(question: &Question, answer: &Answer) -> anyhow::Result<()> {
    let key = question.key();
    if !question.is_collectable() {
        anyhow::bail!("question {:?} cannot be sealed", key);
    }
    if !answer_fits_question(answer, question) {
        anyhow::bail!("answer does not fit question {:?}", key);
    }
    Ok(())
}

pub fn seal_with_deps(payload: Option<&SealedApprovals>, deps: &[String]) -> SealedApprovals {
    let mut ordered: Vec<String> = deps.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let base = payload.cloned().unwrap_or(SealedApprovals {
        answers: Vec::new(),
        proceed: true,
        deps: Vec::new(),
    });
    SealedApprovals {
        deps: ordered,
        ..base
    }
}

pub fn validate(sealed: &SealedApprovals) -> anyhow::Result<()> {
    let mut previous: Option<&QuestionKey> = None;
    for (key, answer) in &sealed.answers {
        if !is_collectable_key(key) {
            anyhow::bail!("question {:?} cannot be sealed", key);
        }
        if !answer_fits_key(answer, key) {
            anyhow::bail!("answer does not fit question {:?}", key);
        }
        if let Some(prev) = previous {
            match key.cmp(prev) {
                std::cmp::Ordering::Less => {
                    anyhow::bail!("answers are not sorted at question {:?}", key)
                }
                std::cmp::Ordering::Equal => {
                    anyhow::bail!("duplicate answers for question {:?}", key)
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        previous = Some(key);
    }
    for pair in sealed.deps.windows(2) {
        match pair[0].cmp(&pair[1]) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => anyhow::bail!("duplicate dep {:?}", pair[0]),
            std::cmp::Ordering::Greater => anyhow::bail!("deps are not sorted at {:?}", pair[1]),
        }
    }
    Ok(())
}

fn is_collectable_key(key: &QuestionKey) -> bool {
    !matches!(
        key,
        QuestionKey::Proceed | QuestionKey::Corrupted { .. } | QuestionKey::ImportKey { .. }
    )
}

fn answer_fits_key(answer: &Answer, key: &QuestionKey) -> bool {
    match (key, answer) {
        (
            QuestionKey::Conflict { first, second },
            Answer::Conflict {
                incoming,
                removable,
                ..
            },
        ) => first <= second && is_same_pair(first, second, incoming, removable),
        (QuestionKey::SelectProvider { .. }, Answer::SelectProvider { .. }) => true,
        (
            QuestionKey::Replace { old, new },
            Answer::Replace {
                old: a_old,
                new: a_new,
                ..
            },
        ) => old == a_old && new == a_new,
        (QuestionKey::InstallIgnorepkg { name }, Answer::InstallIgnorepkg { name: a_name, .. }) => {
            name == a_name
        }
        (QuestionKey::RemovePkgs { names }, Answer::RemovePkgs { names: a_names, .. }) => {
            let mut ordered = a_names.clone();
            ordered.sort();
            ordered == *names
        }
        (QuestionKey::HoldPkgs { names }, Answer::HoldPkgs { names: a_names, .. }) => {
            let mut ordered = a_names.clone();
            ordered.sort();
            ordered == *names
        }
        (QuestionKey::GroupMembers { .. }, Answer::GroupMembers { .. }) => true,
        _ => false,
    }
}

fn answer_fits_question(answer: &Answer, question: &Question) -> bool {
    if !answer_fits_key(answer, &question.key()) {
        return false;
    }
    match (question, answer) {
        (Question::SelectProvider { candidates, .. }, Answer::SelectProvider { name, repo }) => {
            candidates
                .iter()
                .any(|c| c.name == *name && (repo.is_none() || c.repo == *repo))
        }
        (Question::GroupMembers { members, .. }, Answer::GroupMembers { selected }) => {
            selected.iter().all(|name| members.contains(name))
        }
        _ => true,
    }
}

fn is_same_pair(first_a: &str, second_a: &str, first_b: &str, second_b: &str) -> bool {
    (first_a == first_b && second_a == second_b) || (first_a == second_b && second_a == first_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TransactionSummary;
    use crate::question::model::ProviderCandidate;
    use crate::question::model::TransactionKind;
    use serde_json::{from_value, to_value};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    fn candidate(name: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: s(name),
            repo: Some(s("extra")),
            version: None,
        }
    }

    fn collectable_fixture() -> Vec<(Question, Answer)> {
        vec![
            (
                Question::Conflict {
                    incoming: s("foo"),
                    incoming_version: "1.0-1".to_string(),
                    removable: s("bar"),
                    removable_version: "1.0-1".to_string(),
                    conflict_reason: None,
                },
                Answer::Conflict {
                    incoming: s("foo"),
                    removable: s("bar"),
                    remove: true,
                },
            ),
            (
                Question::SelectProvider {
                    depend: s("sdl"),
                    candidates: vec![candidate("sdl12-compat"), candidate("sdl2")],
                },
                Answer::SelectProvider {
                    name: s("sdl12-compat"),
                    repo: Some(s("extra")),
                },
            ),
            (
                Question::GroupMembers {
                    group: s("base-devel"),
                    members: vec![s("autoconf"), s("automake")],
                },
                Answer::GroupMembers {
                    selected: vec![s("automake")],
                },
            ),
            (
                Question::Replace {
                    old: s("nginx"),
                    new: s("nginx-mainline"),
                    repo: None,
                },
                Answer::Replace {
                    old: s("nginx"),
                    new: s("nginx-mainline"),
                    replace: true,
                },
            ),
            (
                Question::InstallIgnorepkg { name: s("glibc") },
                Answer::InstallIgnorepkg {
                    name: s("glibc"),
                    install: true,
                },
            ),
            (
                Question::RemovePkgs {
                    names: vec![s("b"), s("a")],
                    kind: TransactionKind::Install,
                },
                Answer::RemovePkgs {
                    names: vec![s("a"), s("b")],
                    skip: false,
                },
            ),
            (
                Question::HoldPkgs {
                    names: vec![s("b"), s("a")],
                },
                Answer::HoldPkgs {
                    names: vec![s("a"), s("b")],
                    proceed: true,
                },
            ),
        ]
    }

    fn split(fixture: &[(Question, Answer)]) -> (Vec<Question>, Vec<Answer>) {
        fixture.iter().cloned().unzip()
    }

    #[test]
    fn seal_round_trips_sorted_and_deterministic() {
        let fixture = collectable_fixture();
        let (questions, answers) = split(&fixture);
        let sealed = seal(&questions, &answers, true).expect("seal succeeds");
        assert!(sealed.proceed);
        let keys: Vec<&QuestionKey> = sealed.answers.iter().map(|(key, _)| key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        let restored: SealedApprovals =
            from_value(to_value(&sealed).expect("serializes")).expect("deserializes");
        assert_eq!(restored, sealed);
        let (rev_questions, rev_answers) =
            split(&fixture.iter().cloned().rev().collect::<Vec<_>>());
        assert_eq!(
            seal(&rev_questions, &rev_answers, false)
                .expect("reordered seal succeeds")
                .answers,
            seal(&questions, &answers, false)
                .expect("seal succeeds")
                .answers
        );
    }

    #[test]
    fn seal_rejects_mismatches_and_unsealable() {
        let fixture = collectable_fixture();
        let (questions, answers) = split(&fixture);
        assert!(seal(&questions[..1], &[], false).is_err());
        assert!(seal(&questions[..1], &answers[4..5], false).is_err());
        assert!(
            seal(
                &[Question::Corrupted { path: s("p") }],
                &[answers[4].clone()],
                false
            )
            .is_err()
        );
        assert!(
            seal(
                &[Question::Proceed {
                    summary: summary(),
                    kind: TransactionKind::Install,
                }],
                &[answers[4].clone()],
                false
            )
            .is_err()
        );
        assert!(
            seal(
                &[questions[0].clone()],
                &[Answer::Conflict {
                    incoming: s("foo"),
                    removable: s("baz"),
                    remove: true
                }],
                false
            )
            .is_err()
        );
        assert!(
            seal(
                &[questions[1].clone()],
                &[Answer::SelectProvider {
                    name: s("sdl3"),
                    repo: None
                }],
                false
            )
            .is_err()
        );
        let dup = vec![questions[1].clone(), questions[1].clone()];
        assert!(
            seal(
                &dup,
                &[
                    answers[1].clone(),
                    Answer::SelectProvider {
                        name: s("sdl2"),
                        repo: Some(s("extra"))
                    }
                ],
                false
            )
            .is_err()
        );
        assert_eq!(
            seal(&dup, &[answers[1].clone(), answers[1].clone()], false)
                .expect("identical re-seal succeeds")
                .answers
                .len(),
            1
        );
        assert!(
            seal(
                &[Question::GroupMembers {
                    group: s("tools"),
                    members: vec![s("a"), s("b")],
                }],
                &[Answer::GroupMembers {
                    selected: vec![s("c")],
                }],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn replay_returns_stored_or_asks() {
        let fixture = collectable_fixture();
        let (questions, answers) = split(&fixture);
        let sealed = seal(&questions, &answers, true).expect("seal succeeds");
        for (question, answer) in &fixture {
            assert_eq!(
                match_answer(question, &sealed),
                Sealed::Answer(answer.clone())
            );
        }
        assert_eq!(
            match_answer(
                &Question::SelectProvider {
                    depend: s("libgl"),
                    candidates: vec![]
                },
                &sealed
            ),
            Sealed::Ask
        );
        let shrunk = Question::SelectProvider {
            depend: s("sdl"),
            candidates: vec![candidate("sdl2")],
        };
        assert_eq!(match_answer(&shrunk, &sealed), Sealed::Ask);
        let repo_less = Question::SelectProvider {
            depend: s("ffmpeg"),
            candidates: vec![ProviderCandidate {
                name: s("ffmpeg-full"),
                repo: Some(s("core")),
                version: None,
            }],
        };
        let repo_less_answer = Answer::SelectProvider {
            name: s("ffmpeg-full"),
            repo: None,
        };
        let repo_less_sealed = seal(
            std::slice::from_ref(&repo_less),
            std::slice::from_ref(&repo_less_answer),
            false,
        )
        .expect("repo-less answer seals");
        assert_eq!(
            match_answer(&repo_less, &repo_less_sealed),
            Sealed::Answer(repo_less_answer)
        );
    }

    #[test]
    fn empty_proceed_seal_answers_proceed_and_defers_the_rest() {
        let sealed = SealedApprovals {
            answers: Vec::new(),
            proceed: true,
            deps: Vec::new(),
        };
        validate(&sealed).expect("empty proceed seal is valid");
        assert_eq!(
            match_answer(
                &Question::Proceed {
                    summary: summary(),
                    kind: TransactionKind::Install,
                },
                &sealed
            ),
            Sealed::Answer(Answer::Proceed)
        );
        assert_eq!(
            match_answer(
                &Question::Conflict {
                    incoming: s("foo"),
                    incoming_version: "1.0-1".to_string(),
                    removable: s("bar"),
                    removable_version: "1.0-1".to_string(),
                    conflict_reason: None,
                },
                &sealed
            ),
            Sealed::Ask
        );
    }

    #[test]
    fn defers_runtime_and_proceed_and_normalizes_key_order() {
        let fixture = collectable_fixture();
        let (questions, answers) = split(&fixture);
        let sealed = seal(&questions, &answers, true).expect("seal succeeds");
        let proceed = Question::Proceed {
            summary: summary(),
            kind: TransactionKind::Install,
        };
        assert_eq!(
            match_answer(&proceed, &sealed),
            Sealed::Answer(Answer::Proceed)
        );
        assert_eq!(
            match_answer(
                &proceed,
                &seal(&questions, &answers, false).expect("seal succeeds")
            ),
            Sealed::Ask
        );
        assert_eq!(
            match_answer(&Question::Corrupted { path: s("p") }, &sealed),
            Sealed::Ask
        );
        assert_eq!(
            match_answer(
                &Question::ImportKey {
                    fingerprint: s("f"),
                    uid: s("u")
                },
                &sealed
            ),
            Sealed::Ask
        );
        let swapped = Question::Conflict {
            incoming: s("bar"),
            incoming_version: "1.0-1".to_string(),
            removable: s("foo"),
            removable_version: "1.0-1".to_string(),
            conflict_reason: None,
        };
        assert_eq!(
            match_answer(&swapped, &sealed),
            Sealed::Answer(fixture[0].1.clone())
        );
        let reordered = Question::RemovePkgs {
            names: vec![s("a"), s("b")],
            kind: TransactionKind::Install,
        };
        assert_eq!(
            match_answer(&reordered, &sealed),
            Sealed::Answer(fixture[5].1.clone())
        );
    }

    #[test]
    fn dep_seals_stay_sorted_unique_and_legacy_compatible() {
        for deps in [vec![s("b"), s("a")], vec![s("a"), s("a")]] {
            assert!(
                validate(&SealedApprovals {
                    answers: Vec::new(),
                    proceed: true,
                    deps
                })
                .is_err()
            );
        }
        validate(&SealedApprovals {
            answers: Vec::new(),
            proceed: true,
            deps: vec![s("a"), s("b")],
        })
        .expect("sorted unique deps are valid");
        let composed = seal_with_deps(None, &[s("b"), s("a"), s("b")]);
        assert_eq!(composed.deps, vec![s("a"), s("b")]);
        assert!(composed.proceed && composed.answers.is_empty());
        let legacy = serde_json::json!({"answers": [], "proceed": true});
        let decoded: SealedApprovals = from_value(legacy).expect("legacy seal decodes");
        assert!(decoded.deps.is_empty());
        validate(&decoded).expect("legacy seal validates");
    }

    #[test]
    fn seal_with_deps_preserves_payload_answers_and_proceed() {
        let fixture = collectable_fixture();
        let (questions, answers) = split(&fixture);
        let payload = seal(&questions, &answers, false).expect("seal succeeds");
        let composed = seal_with_deps(Some(&payload), &[s("lib")]);
        assert_eq!(composed.answers, payload.answers);
        assert!(!composed.proceed);
        assert_eq!(composed.deps, vec![s("lib")]);
        validate(&composed).expect("composed seal validates");
    }
}
