use std::collections::{BTreeMap, BTreeSet};

use super::model::{Answer, Question, QuestionKey};
use super::review::{ReviewDelta, fingerprint, summary_delta};
use crate::events::TransactionSummary;

pub const MAX_REVALIDATIONS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDrift {
    pub added: BTreeSet<QuestionKey>,
    pub changed: BTreeSet<QuestionKey>,
    pub removed: BTreeSet<QuestionKey>,
    pub summary: ReviewDelta,
}

impl ReviewDrift {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.changed.is_empty()
            && self.removed.is_empty()
            && self.summary.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revalidation {
    pub converged: bool,
    pub drift: ReviewDrift,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Converged,
    Diverged(ReviewDrift),
    Unstable,
}

pub fn revalidate(
    approved_questions: &[Question],
    approved_answers: &[Answer],
    approved_summary: &TransactionSummary,
    fresh_questions: &[Question],
    fresh_answers: &[Answer],
    fresh_summary: &TransactionSummary,
) -> Revalidation {
    let converged = fingerprint(approved_questions, approved_answers, approved_summary)
        == fingerprint(fresh_questions, fresh_answers, fresh_summary);
    let approved_map = keyed(approved_questions);
    let fresh_map = keyed(fresh_questions);
    let added = fresh_map
        .keys()
        .filter(|key| !approved_map.contains_key(*key))
        .cloned()
        .collect();
    let changed = fresh_map
        .iter()
        .filter(|(key, fresh)| {
            approved_map
                .get(*key)
                .is_some_and(|approved| *approved != **fresh)
        })
        .map(|(key, _)| key.clone())
        .collect();
    let removed = approved_map
        .keys()
        .filter(|key| !fresh_map.contains_key(*key))
        .cloned()
        .collect();
    Revalidation {
        converged,
        drift: ReviewDrift {
            added,
            changed,
            removed,
            summary: summary_delta(approved_summary, fresh_summary),
        },
    }
}

pub fn converge(diverged_rounds: usize, outcome: &Revalidation) -> Verdict {
    if outcome.converged {
        return Verdict::Converged;
    }
    if diverged_rounds < MAX_REVALIDATIONS {
        return Verdict::Diverged(outcome.drift.clone());
    }
    Verdict::Unstable
}

fn keyed(questions: &[Question]) -> BTreeMap<QuestionKey, &Question> {
    debug_assert!(
        {
            let mut seen = BTreeSet::new();
            questions.iter().all(|question| seen.insert(question.key()))
        },
        "keyed expects {} questions with distinct keys",
        questions.len()
    );
    questions
        .iter()
        .map(|question| (question.key(), question))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SummaryPackage;
    use crate::question::model::ProviderCandidate;

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

    fn base_summary() -> TransactionSummary {
        summary(vec![package("alpha", "1.0")])
    }

    fn conflict_state() -> (Vec<Question>, Vec<Answer>) {
        (
            vec![Question::Conflict {
                incoming: "cava-git".to_string(),
                incoming_version: "1.0-1".to_string(),
                removable: "cava".to_string(),
                removable_version: "1.0-1".to_string(),
                conflict_reason: None,
            }],
            vec![Answer::Conflict {
                incoming: "cava-git".to_string(),
                removable: "cava".to_string(),
                remove: true,
            }],
        )
    }

    fn provider(candidates: Vec<&str>) -> Question {
        Question::SelectProvider {
            depend: "virt".to_string(),
            candidates: candidates
                .into_iter()
                .map(|name| ProviderCandidate {
                    name: name.to_string(),
                    repo: Some("extra".to_string()),
                    version: None,
                })
                .collect(),
        }
    }

    fn provider_answer(name: &str) -> Answer {
        Answer::SelectProvider {
            name: name.to_string(),
            repo: Some("extra".to_string()),
        }
    }

    fn ignorepkg(name: &str) -> Question {
        Question::InstallIgnorepkg {
            name: name.to_string(),
        }
    }

    fn ignorepkg_answer(name: &str) -> Answer {
        Answer::InstallIgnorepkg {
            name: name.to_string(),
            install: false,
        }
    }

    #[test]
    fn provider_flip_surfaces_delta_then_converges() {
        let (approved_questions, approved_answers) = conflict_state();
        let drifted = revalidate(
            &approved_questions,
            &approved_answers,
            &base_summary(),
            &[provider(vec!["qemu", "kvm"])],
            &[provider_answer("qemu")],
            &summary(vec![package("alpha", "1.0"), package("beta", "3.0")]),
        );
        assert!(!drifted.converged);
        assert_eq!(drifted.drift.added.len(), 1);
        assert_eq!(drifted.drift.removed.len(), 1);
        assert!(!drifted.drift.summary.is_empty());
        assert!(matches!(converge(0, &drifted), Verdict::Diverged(_)));
        assert!(matches!(converge(1, &drifted), Verdict::Diverged(_)));

        let settled = revalidate(
            &[provider(vec!["qemu", "kvm"])],
            &[provider_answer("qemu")],
            &summary(vec![package("alpha", "1.0"), package("beta", "3.0")]),
            &[provider(vec!["qemu", "kvm"])],
            &[provider_answer("qemu")],
            &summary(vec![package("alpha", "1.0"), package("beta", "3.0")]),
        );
        assert!(settled.converged);
        assert!(settled.drift.is_empty());
        assert!(matches!(converge(1, &settled), Verdict::Converged));
    }

    #[test]
    fn repeated_drift_goes_unstable_after_two_revalidations() {
        let (approved_questions, approved_answers) = conflict_state();
        let diverging = revalidate(
            &approved_questions,
            &approved_answers,
            &base_summary(),
            &[ignorepkg("glibc")],
            &[ignorepkg_answer("glibc")],
            &base_summary(),
        );
        assert!(!diverging.converged);
        assert!(matches!(converge(0, &diverging), Verdict::Diverged(_)));
        assert!(matches!(converge(1, &diverging), Verdict::Diverged(_)));
        assert!(matches!(converge(2, &diverging), Verdict::Unstable));
    }
}
