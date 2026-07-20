use anyhow::{Context as _, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Conflict {
    pub incoming: String,
    pub removable: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderCandidate {
    pub name: String,
    pub repo: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl QuestionSet {
    pub fn approve(
        &self,
        conflict_selections: &[usize],
        provider_choices: &[(usize, usize)],
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
        Ok(Approvals {
            approved_conflicts,
            approved_providers,
        })
    }
}

pub fn encode_approvals(approvals: &Approvals) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(approvals).context("failed to serialize approvals")?;
    Ok(STANDARD.encode(&bytes))
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
        }
    }

    #[test]
    fn approve_round_trips_conflicts_and_providers() {
        let qs = sample_question_set();
        let approvals = qs.approve(&[0], &[(0, 1), (1, 0)]).expect("approve");

        let b64 = encode_approvals(&approvals).expect("encode");
        let bytes = STANDARD.decode(&b64).expect("decode");
        let decoded: Approvals = serde_json::from_slice(&bytes).expect("deserialize");

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
        let err = qs.approve(&[5], &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn approve_rejects_out_of_range_candidate_index() {
        let qs = sample_question_set();
        let err = qs.approve(&[], &[(0, 99)]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn approve_rejects_out_of_range_prompt_index() {
        let qs = sample_question_set();
        let err = qs.approve(&[], &[(99, 0)]).unwrap_err();
        assert!(
            format!("{err:#}").contains("out of range"),
            "expected out-of-range message, got: {err:#}"
        );
    }

    #[test]
    fn decode_legacy_approvals_without_providers() {
        let legacy = r#"{"approved_conflicts":[{"incoming":"cava-git","removable":"cava"}]}"#;
        let decoded: Approvals = serde_json::from_str(legacy).expect("decode legacy");
        assert!(decoded.approved_providers.is_empty());
        assert_eq!(decoded.approved_conflicts.len(), 1);
    }
}
