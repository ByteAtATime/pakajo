use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{Answer, Question, QuestionKey};
use crate::events::{SummaryPackage, TransactionSummary, classify_action, target_version};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub questions: BTreeSet<(QuestionKey, String)>,
    pub summary: BTreeSet<(String, String, String)>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Review {
    pub part1: Vec<Question>,
    pub part2: TransactionSummary,
    pub generated_by: ExploreStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExploreStamp {
    pub dbs: BTreeMap<String, DbMark>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbMark {
    pub packages: usize,
    pub mtime: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDelta {
    pub added: BTreeSet<(String, String, String)>,
    pub removed: BTreeSet<(String, String, String)>,
}

pub fn fingerprint(
    questions: &[Question],
    answers: &[Answer],
    summary: &TransactionSummary,
) -> Fingerprint {
    Fingerprint {
        questions: answered_set(questions, answers),
        summary: summary_set(summary),
    }
}

pub fn summary_delta(previous: &TransactionSummary, current: &TransactionSummary) -> ReviewDelta {
    let before = summary_set(previous);
    let after = summary_set(current);
    ReviewDelta {
        added: after.difference(&before).cloned().collect(),
        removed: before.difference(&after).cloned().collect(),
    }
}

impl ReviewDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

fn answered_set(questions: &[Question], answers: &[Answer]) -> BTreeSet<(QuestionKey, String)> {
    debug_assert!(
        questions.len() == answers.len(),
        "answered_set expects {} questions and {} answers to align",
        questions.len(),
        answers.len()
    );
    questions
        .iter()
        .zip(answers.iter())
        .map(|(question, answer)| (question.key(), answer_tag(answer)))
        .collect()
}

fn summary_set(summary: &TransactionSummary) -> BTreeSet<(String, String, String)> {
    summary.packages.iter().map(summary_entry).collect()
}

fn summary_entry(pkg: &SummaryPackage) -> (String, String, String) {
    (
        pkg.name.clone(),
        format!("{:?}", classify_action(pkg)),
        target_version(pkg).to_string(),
    )
}

fn answer_tag(answer: &Answer) -> String {
    match answer {
        Answer::Conflict { remove, .. } => flag("conflict", *remove, "remove", "keep"),
        Answer::SelectProvider { name, repo } => {
            format!("provider:{}:{}", name, repo.as_deref().unwrap_or_default())
        }
        Answer::Replace { replace, .. } => flag("replace", *replace, "replace", "keep"),
        Answer::InstallIgnorepkg { install, .. } => flag("ignorepkg", *install, "install", "skip"),
        Answer::RemovePkgs { skip, .. } => flag("removepkgs", *skip, "skip", "keep"),
        Answer::HoldPkgs { proceed, .. } => flag("holdpkgs", *proceed, "continue", "abort"),
        Answer::Corrupted { remove, .. } => flag("corrupted", *remove, "delete", "keep"),
        Answer::ImportKey { import, .. } => flag("importkey", *import, "import", "reject"),
        Answer::Proceed => "proceed".to_string(),
        Answer::Stop => "stop".to_string(),
        Answer::GroupMembers { selected } => group_tag(selected),
    }
}

fn flag(kind: &str, set: bool, when_set: &str, when_unset: &str) -> String {
    format!("{}:{}", kind, if set { when_set } else { when_unset })
}

fn group_tag(selected: &[String]) -> String {
    let mut ordered = selected.to_vec();
    ordered.sort();
    format!("members:{}", ordered.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SummaryPackage;

    fn package(name: &str, version: &str) -> SummaryPackage {
        SummaryPackage {
            name: name.to_string(),
            repository: None,
            new_version: version.to_string(),
            old_version: None,
            download_size: 0,
            installed_size: 0,
            old_installed_size: 0,
            is_removal: false,
        }
    }

    fn summary(packages: Vec<SummaryPackage>) -> TransactionSummary {
        TransactionSummary {
            packages,
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    fn conflict_pair() -> (Question, Answer) {
        (
            Question::Conflict {
                incoming: "cava-git".to_string(),
                incoming_version: "1.0-1".to_string(),
                removable: "cava".to_string(),
                removable_version: "1.0-1".to_string(),
                conflict_reason: None,
            },
            Answer::Conflict {
                incoming: "cava-git".to_string(),
                removable: "cava".to_string(),
                remove: true,
            },
        )
    }

    #[test]
    fn fingerprint_matches_hand_computed_value_and_detects_version_bump() {
        let (question, answer) = conflict_pair();
        let got = fingerprint(
            &[question],
            &[answer],
            &summary(vec![package("alpha", "1.0")]),
        );
        let mut questions = BTreeSet::new();
        questions.insert((
            QuestionKey::Conflict {
                first: "cava".to_string(),
                second: "cava-git".to_string(),
            },
            "conflict:remove".to_string(),
        ));
        let mut entries = BTreeSet::new();
        entries.insert((
            "alpha".to_string(),
            "Install".to_string(),
            "1.0".to_string(),
        ));
        assert_eq!(
            got,
            Fingerprint {
                questions,
                summary: entries,
            }
        );
        let (repeat_q, repeat_a) = conflict_pair();
        let same = fingerprint(
            &[repeat_q],
            &[repeat_a],
            &summary(vec![package("alpha", "1.0")]),
        );
        assert_eq!(got, same);
        let (bump_q, bump_a) = conflict_pair();
        let bumped = fingerprint(
            &[bump_q],
            &[bump_a],
            &summary(vec![package("alpha", "2.0")]),
        );
        assert_ne!(got, bumped);
    }

    #[test]
    fn review_and_stamp_round_trip_and_compare() {
        let (question, _) = conflict_pair();
        let review = Review {
            part1: vec![question],
            part2: summary(vec![package("alpha", "1.0")]),
            generated_by: ExploreStamp {
                dbs: BTreeMap::from([(
                    "core".to_string(),
                    DbMark {
                        packages: 799,
                        mtime: 1_700_000,
                    },
                )]),
            },
        };
        let json = serde_json::to_string(&review).unwrap();
        assert_eq!(serde_json::from_str::<Review>(&json).unwrap(), review);

        let mut other = review.clone();
        other.generated_by.dbs.get_mut("core").unwrap().mtime += 1;
        assert_ne!(review.generated_by, other.generated_by);
    }

    #[test]
    fn delta_names_added_and_removed_packages() {
        let got = summary_delta(
            &summary(vec![package("alpha", "1.0")]),
            &summary(vec![package("alpha", "1.0"), package("beta", "3.0")]),
        );
        let mut added = BTreeSet::new();
        added.insert(("beta".to_string(), "Install".to_string(), "3.0".to_string()));
        assert_eq!(
            got,
            ReviewDelta {
                added,
                removed: BTreeSet::new(),
            }
        );
        assert!(
            summary_delta(
                &summary(vec![package("alpha", "1.0")]),
                &summary(vec![package("alpha", "1.0")]),
            )
            .is_empty()
        );
    }
}
