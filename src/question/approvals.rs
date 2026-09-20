use super::legacy::{Approvals, Conflict, ProviderApproval};
use super::model::{Answer, ProviderCandidate, Question};

#[derive(Debug, Clone)]
pub enum Sealed {
    Answer(Answer),
    Ask,
}

pub fn seal(questions: &[Question], answers: &[Answer]) -> Approvals {
    let mut sealed = Approvals::default();
    for (question, answer) in questions.iter().zip(answers.iter()) {
        extend(&mut sealed, question, answer);
    }
    sealed
}

pub fn extend(approvals: &mut Approvals, question: &Question, answer: &Answer) -> bool {
    match (question, answer) {
        (
            Question::Conflict {
                incoming,
                removable,
            },
            Answer::Conflict {
                incoming: accept_incoming,
                removable: accept_removable,
                remove: true,
            },
        ) => extend_conflict(
            approvals,
            incoming,
            removable,
            accept_incoming,
            accept_removable,
        ),
        (
            Question::SelectProvider { depend, candidates },
            Answer::SelectProvider { name, repo },
        ) => extend_provider(approvals, depend, candidates, name, repo),
        (Question::GroupMembers { group, members }, Answer::GroupMembers { selected }) => {
            extend_group_members(approvals, group, members, selected)
        }
        _ => false,
    }
}

pub fn match_answer(question: &Question, approvals: &Approvals) -> Sealed {
    match question {
        Question::Conflict {
            incoming,
            removable,
        } => match_conflict(approvals, incoming, removable),
        Question::SelectProvider { depend, candidates } => {
            match_provider(approvals, depend, candidates)
        }
        Question::GroupMembers { group, members } => match_group_members(approvals, group, members),
        Question::Proceed(_) => Sealed::Ask,
        _ => Sealed::Ask,
    }
}

fn is_same_pair(first_a: &str, second_a: &str, first_b: &str, second_b: &str) -> bool {
    (first_a == first_b && second_a == second_b) || (first_a == second_b && second_a == first_b)
}

fn extend_conflict(
    approvals: &mut Approvals,
    incoming: &str,
    removable: &str,
    accept_incoming: &str,
    accept_removable: &str,
) -> bool {
    if !is_same_pair(incoming, removable, accept_incoming, accept_removable) {
        return false;
    }
    if approvals
        .approved_conflicts
        .iter()
        .any(|known| is_same_pair(&known.incoming, &known.removable, incoming, removable))
    {
        return true;
    }
    approvals.approved_conflicts.push(Conflict {
        incoming: incoming.to_string(),
        removable: removable.to_string(),
    });
    true
}

fn extend_provider(
    approvals: &mut Approvals,
    depend: &str,
    candidates: &[ProviderCandidate],
    name: &str,
    repo: &Option<String>,
) -> bool {
    if !candidates
        .iter()
        .any(|candidate| candidate.name == name && candidate.repo == *repo)
    {
        return false;
    }
    if approvals
        .approved_providers
        .iter()
        .any(|known| known.depend == depend)
    {
        return true;
    }
    approvals.approved_providers.push(ProviderApproval {
        depend: depend.to_string(),
        provider_name: name.to_string(),
        provider_repo: repo.clone(),
    });
    true
}

fn extend_group_members(
    approvals: &mut Approvals,
    group: &str,
    members: &[String],
    selected: &[String],
) -> bool {
    if !selected.iter().all(|name| members.contains(name)) {
        return false;
    }
    let entry = approvals
        .approved_groups
        .entry(group.to_string())
        .or_default();
    for name in selected {
        if !entry.contains(name) {
            entry.push(name.clone());
        }
    }
    entry.sort();
    true
}

fn match_conflict(approvals: &Approvals, incoming: &str, removable: &str) -> Sealed {
    let approved = approvals
        .approved_conflicts
        .iter()
        .any(|known| is_same_pair(&known.incoming, &known.removable, incoming, removable));
    if !approved {
        return Sealed::Ask;
    }
    Sealed::Answer(Answer::Conflict {
        incoming: incoming.to_string(),
        removable: removable.to_string(),
        remove: true,
    })
}

fn match_provider(approvals: &Approvals, depend: &str, candidates: &[ProviderCandidate]) -> Sealed {
    let Some(known) = approvals
        .approved_providers
        .iter()
        .find(|known| known.depend == depend)
    else {
        return Sealed::Ask;
    };
    let offered = match &known.provider_repo {
        Some(repo) => candidates.iter().any(|candidate| {
            candidate.name == known.provider_name
                && candidate.repo.as_deref() == Some(repo.as_str())
        }),
        None => candidates
            .iter()
            .any(|candidate| candidate.name == known.provider_name),
    };
    if !offered {
        return Sealed::Ask;
    }
    Sealed::Answer(Answer::SelectProvider {
        name: known.provider_name.clone(),
        repo: known.provider_repo.clone(),
    })
}

fn match_group_members(approvals: &Approvals, group: &str, members: &[String]) -> Sealed {
    let Some(selected) = approvals.approved_groups.get(group) else {
        return Sealed::Ask;
    };
    if !selected.iter().all(|name| members.contains(name)) {
        return Sealed::Ask;
    }
    Sealed::Answer(Answer::GroupMembers {
        selected: selected.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TransactionSummary;

    fn summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    fn provider_question() -> Question {
        Question::SelectProvider {
            depend: "sdl".to_string(),
            candidates: vec![
                ProviderCandidate {
                    name: "sdl12-compat".to_string(),
                    repo: Some("extra".to_string()),
                    version: None,
                },
                ProviderCandidate {
                    name: "sdl2".to_string(),
                    repo: Some("extra".to_string()),
                    version: None,
                },
            ],
        }
    }

    fn provider_answer() -> Answer {
        Answer::SelectProvider {
            name: "sdl12-compat".to_string(),
            repo: Some("extra".to_string()),
        }
    }

    fn conflict_question() -> Question {
        Question::Conflict {
            incoming: "foo".to_string(),
            removable: "bar".to_string(),
        }
    }

    fn conflict_answer(remove: bool) -> Answer {
        Answer::Conflict {
            incoming: "foo".to_string(),
            removable: "bar".to_string(),
            remove,
        }
    }

    #[test]
    fn sealed_provider_matches_only_its_own_question() {
        let sealed = seal(&[provider_question()], &[provider_answer()]);
        assert!(matches!(
            match_answer(&provider_question(), &sealed),
            Sealed::Answer(Answer::SelectProvider { name, .. })
            if name == "sdl12-compat"
        ));
        let stranger = Question::SelectProvider {
            depend: "libgl".to_string(),
            candidates: vec![],
        };
        assert!(matches!(match_answer(&stranger, &sealed), Sealed::Ask));
    }

    #[test]
    fn approved_groups_survive_round_trip_and_default_empty() {
        let question = Question::GroupMembers {
            group: "base-devel".to_string(),
            members: vec!["autoconf".to_string()],
        };
        let answer = Answer::GroupMembers {
            selected: vec!["autoconf".to_string()],
        };
        let sealed = seal(&[question], &[answer]);
        let json = serde_json::to_string(&sealed).expect("encode");
        let decoded: Approvals = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, sealed);
        let legacy: Approvals =
            serde_json::from_str(r#"{"approved_conflicts":[]}"#).expect("decode legacy");
        assert!(legacy.approved_groups.is_empty());
    }

    #[test]
    fn unmatched_collectable_question_asks_instead_of_guessing() {
        assert!(matches!(
            match_answer(&provider_question(), &Approvals::default()),
            Sealed::Ask
        ));
    }

    #[test]
    fn proceed_without_sealed_confirmation_does_not_pass() {
        let question = Question::Proceed(summary());
        assert!(matches!(
            match_answer(&question, &Approvals::default()),
            Sealed::Ask
        ));
    }

    #[test]
    fn sealed_conflict_matches_swapped_orientation() {
        let sealed = seal(&[conflict_question()], &[conflict_answer(true)]);
        let swapped = Question::Conflict {
            incoming: "bar".to_string(),
            removable: "foo".to_string(),
        };
        assert!(matches!(
            match_answer(&swapped, &sealed),
            Sealed::Answer(Answer::Conflict { incoming, removable, remove: true })
            if incoming == "bar" && removable == "foo"
        ));
    }

    #[test]
    fn declined_conflict_records_nothing() {
        let mut stored = Approvals::default();
        assert!(!extend(
            &mut stored,
            &conflict_question(),
            &conflict_answer(false)
        ));
        assert!(stored.approved_conflicts.is_empty());
    }

    #[test]
    fn group_answer_with_foreign_name_records_nothing() {
        let mut stored = Approvals::default();
        let question = Question::GroupMembers {
            group: "base-devel".to_string(),
            members: vec!["autoconf".to_string()],
        };
        let answer = Answer::GroupMembers {
            selected: vec!["autoconf".to_string(), "stranger".to_string()],
        };
        assert!(!extend(&mut stored, &question, &answer));
        assert!(!stored.approved_groups.contains_key("base-devel"));
    }

    #[test]
    fn provider_answer_with_unoffered_repo_records_nothing() {
        let mut stored = Approvals::default();
        let answer = Answer::SelectProvider {
            name: "sdl12-compat".to_string(),
            repo: Some("aur".to_string()),
        };
        assert!(!extend(&mut stored, &provider_question(), &answer));
        assert!(stored.approved_providers.is_empty());
    }
}
