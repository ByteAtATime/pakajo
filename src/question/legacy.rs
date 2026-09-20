use std::collections::{BTreeMap, HashMap};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

pub use super::model::ProviderCandidate;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Conflict {
    pub incoming: String,
    pub removable: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderPrompt {
    pub depend: String,
    pub candidates: Vec<ProviderCandidate>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestionSet {
    pub conflicts: Vec<Conflict>,
    pub providers: Vec<ProviderPrompt>,
    pub had_unsupported_question: bool,
    pub unsupported_summary: String,
    #[serde(default)]
    pub held: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderApproval {
    pub depend: String,
    pub provider_name: String,
    pub provider_repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Approvals {
    pub approved_conflicts: Vec<Conflict>,
    #[serde(default)]
    pub approved_providers: Vec<ProviderApproval>,
    #[serde(default)]
    pub approved_held: Vec<String>,
    #[serde(default)]
    pub approved_groups: BTreeMap<String, Vec<String>>,
}

impl QuestionSet {
    pub fn approve(
        &self,
        conflict_selections: &[usize],
        provider_choices: &[(usize, usize)],
        held: &[String],
    ) -> anyhow::Result<Approvals> {
        for &i in conflict_selections {
            let Some(_) = self.conflicts.get(i) else {
                bail!(
                    "conflict selection {i} out of range (have {})",
                    self.conflicts.len()
                );
            };
        }
        for &(prompt_index, candidate_index) in provider_choices {
            let Some(prompt) = self.providers.get(prompt_index) else {
                bail!(
                    "provider prompt {prompt_index} out of range (have {})",
                    self.providers.len()
                );
            };
            let Some(_) = prompt.candidates.get(candidate_index) else {
                bail!(
                    "provider candidate {candidate_index} out of range for prompt \"{}\" (have {})",
                    prompt.depend,
                    prompt.candidates.len()
                );
            };
        }

        for name in held {
            if !self.held.contains(name) {
                bail!(
                    "held package \"{name}\" is not pending removal (have {})",
                    self.held.len()
                );
            }
        }
        let approved_conflicts = conflict_selections
            .iter()
            .map(|&i| self.conflicts[i].clone())
            .collect();
        let approved_providers = provider_choices
            .iter()
            .map(|&(prompt_index, candidate_index)| {
                let prompt = &self.providers[prompt_index];
                let candidate = &prompt.candidates[candidate_index];
                ProviderApproval {
                    depend: prompt.depend.clone(),
                    provider_name: candidate.name.clone(),
                    provider_repo: candidate.repo.clone(),
                }
            })
            .collect();
        let approved_held = held.to_vec();
        Ok(Approvals {
            approved_conflicts,
            approved_providers,
            approved_held,
            approved_groups: BTreeMap::new(),
        })
    }
}

pub fn default_approve(qs: &QuestionSet) -> anyhow::Result<Approvals> {
    let conflicts: Vec<usize> = (0..qs.conflicts.len()).collect();
    let providers: Vec<(usize, usize)> = (0..qs.providers.len()).map(|i| (i, 0)).collect();
    qs.approve(&conflicts, &providers, &qs.held)
}

pub fn collect_approvals(
    qs: &QuestionSet,
    conflict_checks: &[bool],
    provider_choices: &HashMap<String, usize>,
    held: &[String],
) -> anyhow::Result<Approvals> {
    let conflict_selections: Vec<usize> = conflict_checks
        .iter()
        .enumerate()
        .filter_map(|(i, &on)| if on { Some(i) } else { None })
        .collect();
    let provider_choices: Vec<(usize, usize)> = qs
        .providers
        .iter()
        .enumerate()
        .filter_map(|(prompt_index, prompt)| {
            let &candidate_index = provider_choices.get(&prompt.depend)?;
            Some((prompt_index, candidate_index))
        })
        .collect();
    qs.approve(&conflict_selections, &provider_choices, held)
}

pub fn encode_approvals(approvals: &Approvals) -> anyhow::Result<String> {
    serde_json::to_string(approvals).context("failed to serialize approvals")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_question_set() -> QuestionSet {
        QuestionSet {
            conflicts: vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }],
            providers: vec![
                ProviderPrompt {
                    depend: "sdl".into(),
                    candidates: vec![
                        ProviderCandidate {
                            name: "sdl12-compat".into(),
                            repo: Some("cachyos-extra-znver4".into()),
                            version: Some("1.2.68-2.1".into()),
                        },
                        ProviderCandidate {
                            name: "sdl12-compat".into(),
                            repo: Some("extra".into()),
                            version: Some("1.2.68-2".into()),
                        },
                    ],
                },
                ProviderPrompt {
                    depend: "libgl".into(),
                    candidates: vec![ProviderCandidate {
                        name: "libglvnd".into(),
                        repo: Some("extra".into()),
                        version: None,
                    }],
                },
            ],
            had_unsupported_question: false,
            unsupported_summary: String::new(),
            held: vec![],
        }
    }

    #[test]
    fn approve_round_trips_conflicts_and_providers() {
        let qs = sample_question_set();
        let approvals = qs.approve(&[0], &[(0, 1), (1, 0)], &[]).expect("approve");

        let json = encode_approvals(&approvals).expect("encode");
        let decoded: Approvals = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded, approvals);
        assert_eq!(
            decoded.approved_conflicts,
            vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }]
        );
        assert_eq!(
            decoded.approved_providers,
            vec![
                ProviderApproval {
                    depend: "sdl".into(),
                    provider_name: "sdl12-compat".into(),
                    provider_repo: Some("extra".into()),
                },
                ProviderApproval {
                    depend: "libgl".into(),
                    provider_name: "libglvnd".into(),
                    provider_repo: Some("extra".into()),
                }
            ]
        );
    }

    #[test]
    fn approve_rejects_out_of_range_conflict() {
        let qs = sample_question_set();
        let err = qs.approve(&[5], &[], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn approve_rejects_out_of_range_candidate_index() {
        let qs = sample_question_set();
        let err = qs.approve(&[], &[(0, 99)], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn approve_rejects_out_of_range_prompt_index() {
        let qs = sample_question_set();
        let err = qs.approve(&[], &[(99, 0)], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn approve_rejects_out_of_range_held() {
        let qs = sample_question_set();
        let err = qs.approve(&[], &[], &["glibc".to_string()]).unwrap_err();
        assert!(
            format!("{err:#}").contains("is not pending removal"),
            "expected not-pending message, got: {err:#}"
        );
    }

    #[test]
    fn decode_legacy_approvals_without_providers() {
        let legacy = r#"{"approved_conflicts":[{"incoming":"cava-git","removable":"cava"}]}"#;
        let decoded: Approvals = serde_json::from_str(legacy).expect("decode legacy");
        assert!(decoded.approved_providers.is_empty());
        assert_eq!(decoded.approved_conflicts.len(), 1);
    }

    #[test]
    fn decode_legacy_approvals_without_held() {
        let legacy = r#"{"approved_conflicts":[],"approved_providers":[]}"#;
        let decoded: Approvals = serde_json::from_str(legacy).expect("decode legacy");
        assert!(decoded.approved_held.is_empty());
        assert!(decoded.approved_conflicts.is_empty());
        assert!(decoded.approved_providers.is_empty());
    }

    #[test]
    fn default_approve_approves_all_conflicts_and_first_provider() {
        let qs = QuestionSet {
            conflicts: vec![
                Conflict {
                    incoming: "cava-git".into(),
                    removable: "cava".into(),
                },
                Conflict {
                    incoming: "nginx-mainline".into(),
                    removable: "nginx".into(),
                },
            ],
            providers: vec![
                ProviderPrompt {
                    depend: "sdl".into(),
                    candidates: vec![
                        ProviderCandidate {
                            name: "sdl12-compat".into(),
                            repo: Some("extra".into()),
                            version: Some("1.2.68-2".into()),
                        },
                        ProviderCandidate {
                            name: "sdl2".into(),
                            repo: Some("extra".into()),
                            version: Some("2.30.0-1".into()),
                        },
                    ],
                },
                ProviderPrompt {
                    depend: "libgl".into(),
                    candidates: vec![
                        ProviderCandidate {
                            name: "libglvnd".into(),
                            repo: Some("extra".into()),
                            version: None,
                        },
                        ProviderCandidate {
                            name: "nvidia-utils".into(),
                            repo: Some("extra".into()),
                            version: None,
                        },
                    ],
                },
            ],
            had_unsupported_question: false,
            unsupported_summary: String::new(),
            held: vec![],
        };

        let approvals = default_approve(&qs).expect("default_approve");

        assert_eq!(approvals.approved_conflicts.len(), 2);
        assert_eq!(approvals.approved_providers.len(), 2);
        assert_eq!(
            approvals.approved_conflicts, qs.conflicts,
            "default_approve should approve every conflict"
        );
        assert_eq!(
            approvals.approved_providers[0].provider_name, qs.providers[0].candidates[0].name,
            "provider 0 should resolve to candidate 0"
        );
        assert_eq!(
            approvals.approved_providers[1].provider_name, qs.providers[1].candidates[0].name,
            "provider 1 should resolve to candidate 0"
        );
    }

    #[test]
    fn collect_approvals_with_all_checked_matches_default_approve() {
        let qs = sample_question_set();
        let conflict_checks = vec![true; qs.conflicts.len()];
        let provider_choices: HashMap<String, usize> = qs
            .providers
            .iter()
            .map(|prompt| (prompt.depend.clone(), 0))
            .collect();

        let collected = collect_approvals(&qs, &conflict_checks, &provider_choices, &[])
            .expect("collect_approvals with all checked");
        let defaulted = default_approve(&qs).expect("default_approve");

        assert_eq!(collected, defaulted);
    }

    #[test]
    fn collect_approvals_with_no_conflicts_emits_empty_conflicts() {
        let qs = sample_question_set();
        let provider_choices: HashMap<String, usize> = qs
            .providers
            .iter()
            .map(|prompt| (prompt.depend.clone(), 0))
            .collect();

        let approvals =
            collect_approvals(&qs, &[], &provider_choices, &[]).expect("collect_approvals empty");

        assert!(
            approvals.approved_conflicts.is_empty(),
            "no conflict checks should produce no approved conflicts"
        );
        assert_eq!(
            approvals.approved_providers.len(),
            qs.providers.len(),
            "providers should still resolve to their defaults"
        );
    }

    #[test]
    fn collect_approvals_picks_named_provider_candidate() {
        let qs = sample_question_set();
        let mut provider_choices: HashMap<String, usize> = qs
            .providers
            .iter()
            .map(|prompt| (prompt.depend.clone(), 0))
            .collect();
        provider_choices.insert("sdl".to_string(), 1);

        let approvals = collect_approvals(&qs, &[true], &provider_choices, &[])
            .expect("collect_approvals with candidate 1");

        let sdl_approval = approvals
            .approved_providers
            .iter()
            .find(|approval| approval.depend == "sdl")
            .expect("sdl approval should exist");
        assert_eq!(
            sdl_approval.provider_name, qs.providers[0].candidates[1].name,
            "sdl should resolve to candidate index 1"
        );
    }
}
