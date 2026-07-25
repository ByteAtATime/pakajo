use std::io::Write as _;

use crate::color;
use crate::question::ProviderCandidate;

pub trait QuestionAnswerer {
    fn answer_conflict(
        &self,
        incoming: &str,
        incoming_version: &str,
        removable: &str,
        removable_version: &str,
    ) -> ConflictDecision;
    fn answer_provider(&self, depend: &str, candidates: &[ProviderCandidate]) -> ProviderDecision;
}

pub enum ConflictDecision {
    Remove,
    Decline,
    CannotPrompt,
}

#[derive(Debug)]
pub enum ProviderDecision {
    Choose(usize),
    Decline,
    CannotPrompt,
}

fn parse_provider_choice(input: &str, candidate_count: usize) -> ProviderDecision {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ProviderDecision::Choose(0);
    }
    let Ok(n) = trimmed.parse::<usize>() else {
        return ProviderDecision::Decline;
    };
    if n >= 1 && n <= candidate_count {
        ProviderDecision::Choose(n - 1)
    } else {
        ProviderDecision::Decline
    }
}

pub struct StdioAnswerer {
    color: bool,
}

impl StdioAnswerer {
    pub fn new() -> Self {
        Self {
            color: color::stderr_color(),
        }
    }
}

impl QuestionAnswerer for StdioAnswerer {
    fn answer_conflict(
        &self,
        incoming: &str,
        incoming_version: &str,
        removable: &str,
        removable_version: &str,
    ) -> ConflictDecision {
        let inc = color::paint(self.color, color::BOLD, incoming);
        let incv = color::paint(self.color, color::VERSION, incoming_version);
        let rem = color::paint(self.color, color::BOLD, removable);
        let remv = color::paint(self.color, color::VERSION, removable_version);
        let msg = format!(
            "{inc}-{incv} and {rem}-{remv} are in conflict. Remove {rem}? [y/N]"
        );
        eprint!("{} ", color::colon(self.color, &msg));
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return ConflictDecision::CannotPrompt;
        }
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => ConflictDecision::Remove,
            _ => ConflictDecision::Decline,
        }
    }

    fn answer_provider(&self, depend: &str, candidates: &[ProviderCandidate]) -> ProviderDecision {
        if candidates.is_empty() {
            return ProviderDecision::Decline;
        }
        eprint!(
            "{}\n",
            color::colon(
                self.color,
                &format!("There are {} providers available for {}:", candidates.len(), depend)
            )
        );
        for (i, c) in candidates.iter().enumerate() {
            let display = match &c.repo {
                Some(repo) => format!("{repo}/{}", c.name),
                None => c.name.clone(),
            };
            eprint!("  [{}] {display}\n", i + 1);
        }
        eprint!(
            "{} ",
            color::colon(self.color, "Enter a number (default=1):")
        );
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return ProviderDecision::CannotPrompt;
        }
        parse_provider_choice(&input, candidates.len())
    }
}

pub struct NonInteractiveAnswerer;

impl QuestionAnswerer for NonInteractiveAnswerer {
    fn answer_conflict(
        &self,
        _incoming: &str,
        _incoming_version: &str,
        _removable: &str,
        _removable_version: &str,
    ) -> ConflictDecision {
        ConflictDecision::CannotPrompt
    }

    fn answer_provider(
        &self,
        _depend: &str,
        _candidates: &[ProviderCandidate],
    ) -> ProviderDecision {
        ProviderDecision::CannotPrompt
    }
}

// mock answerer used for tests
#[allow(dead_code)]
pub struct DenyAllAnswerer;

impl QuestionAnswerer for DenyAllAnswerer {
    fn answer_conflict(
        &self,
        _incoming: &str,
        _incoming_version: &str,
        _removable: &str,
        _removable_version: &str,
    ) -> ConflictDecision {
        ConflictDecision::Decline
    }

    fn answer_provider(
        &self,
        _depend: &str,
        _candidates: &[ProviderCandidate],
    ) -> ProviderDecision {
        ProviderDecision::Decline
    }
}

pub struct ApprovalsAnswerer {
    approvals: crate::question::Approvals,
}

impl ApprovalsAnswerer {
    pub fn new(approvals: crate::question::Approvals) -> Self {
        Self { approvals }
    }
}

impl QuestionAnswerer for ApprovalsAnswerer {
    fn answer_conflict(
        &self,
        incoming: &str,
        _incoming_version: &str,
        removable: &str,
        _removable_version: &str,
    ) -> ConflictDecision {
        let approved = self.approvals.approved_conflicts.iter().any(|c| {
            (c.incoming == incoming && c.removable == removable)
                || (c.incoming == removable && c.removable == incoming)
        });
        if approved {
            ConflictDecision::Remove
        } else {
            ConflictDecision::Decline
        }
    }

    fn answer_provider(&self, depend: &str, candidates: &[ProviderCandidate]) -> ProviderDecision {
        let Some(approval) = self
            .approvals
            .approved_providers
            .iter()
            .find(|a| a.depend == depend)
        else {
            return ProviderDecision::Decline;
        };
        let idx = match &approval.provider_repo {
            Some(required_repo) => candidates.iter().position(|c| {
                c.name == approval.provider_name
                    && c.repo.as_deref() == Some(required_repo.as_str())
            }),
            None => candidates
                .iter()
                .position(|c| c.name == approval.provider_name),
        };
        match idx {
            Some(i) => ProviderDecision::Choose(i),
            None => ProviderDecision::Decline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{Approvals, Conflict, ProviderApproval, ProviderCandidate};

    #[test]
    fn approvals_answerer_matches_in_either_direction() {
        let approvals = Approvals {
            approved_conflicts: vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }],
            approved_providers: vec![],
        };
        let a = ApprovalsAnswerer::new(approvals);
        assert!(matches!(
            a.answer_conflict("cava-git", "", "cava", ""),
            ConflictDecision::Remove
        ));
        assert!(matches!(
            a.answer_conflict("cava", "", "cava-git", ""),
            ConflictDecision::Remove
        ));
        assert!(matches!(
            a.answer_conflict("foo", "", "bar", ""),
            ConflictDecision::Decline
        ));
    }

    #[test]
    fn approvals_base64_round_trips_and_replays() {
        use base64::Engine as _;
        let original = Approvals {
            approved_conflicts: vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }],
            approved_providers: vec![],
        };
        let json = serde_json::to_vec(&original).expect("serialize");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&json);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("decode");
        let decoded: Approvals = serde_json::from_slice(&bytes).expect("deserialize");
        let a = ApprovalsAnswerer::new(decoded);
        assert!(matches!(
            a.answer_conflict("cava-git", "", "cava", ""),
            ConflictDecision::Remove
        ));
    }

    #[test]
    fn parse_provider_choice_picks_default_on_empty() {
        assert!(matches!(
            parse_provider_choice("", 3),
            ProviderDecision::Choose(0)
        ));
    }

    #[test]
    fn parse_provider_choice_first_entry_maps_to_zero_index() {
        assert!(matches!(
            parse_provider_choice("1", 3),
            ProviderDecision::Choose(0)
        ));
    }

    #[test]
    fn parse_provider_choice_last_entry_maps_to_count_minus_one() {
        assert!(matches!(
            parse_provider_choice("3", 3),
            ProviderDecision::Choose(2)
        ));
    }

    #[test]
    fn parse_provider_choice_zero_is_declined() {
        assert!(matches!(
            parse_provider_choice("0", 3),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn parse_provider_choice_past_end_is_declined() {
        assert!(matches!(
            parse_provider_choice("4", 3),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn parse_provider_choice_non_numeric_is_declined() {
        assert!(matches!(
            parse_provider_choice("abc", 3),
            ProviderDecision::Decline
        ));
    }

    fn candidate(name: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: name.into(),
            repo: None,
            version: None,
        }
    }

    #[test]
    fn approvals_answerer_picks_provider_by_name_after_reorder() {
        let approvals = Approvals {
            approved_conflicts: vec![],
            approved_providers: vec![ProviderApproval {
                depend: "sdl".into(),
                provider_name: "B".into(),
                provider_repo: None,
            }],
        };
        let a = ApprovalsAnswerer::new(approvals);

        let reordered = vec![candidate("C"), candidate("B"), candidate("A")];
        match a.answer_provider("sdl", &reordered) {
            ProviderDecision::Choose(i) => assert_eq!(
                i, 1,
                "B is at index 1 in [C, B, A], proving name-based matching"
            ),
            other => panic!("expected Choose, got {other:?}"),
        }
    }

    #[test]
    fn approvals_answerer_declines_when_approved_provider_missing() {
        let approvals = Approvals {
            approved_conflicts: vec![],
            approved_providers: vec![ProviderApproval {
                depend: "sdl".into(),
                provider_name: "B".into(),
                provider_repo: None,
            }],
        };
        let a = ApprovalsAnswerer::new(approvals);

        let no_b = vec![candidate("C"), candidate("A")];
        assert!(matches!(
            a.answer_provider("sdl", &no_b),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn approvals_answerer_declines_when_no_matching_approval() {
        let approvals = Approvals {
            approved_conflicts: vec![],
            approved_providers: vec![ProviderApproval {
                depend: "sdl".into(),
                provider_name: "A".into(),
                provider_repo: None,
            }],
        };
        let a = ApprovalsAnswerer::new(approvals);
        assert!(matches!(
            a.answer_provider("libgl", &[candidate("libglvnd")]),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn answer_provider_disambiguates_same_name_different_repo() {
        let approvals = Approvals {
            approved_conflicts: vec![],
            approved_providers: vec![ProviderApproval {
                depend: "sdl".into(),
                provider_name: "sdl12-compat".into(),
                provider_repo: Some("extra".into()),
            }],
        };
        let a = ApprovalsAnswerer::new(approvals);
        let candidates = vec![
            ProviderCandidate {
                name: "sdl12-compat".into(),
                repo: Some("cachyos-extra-znver4".into()),
                version: None,
            },
            ProviderCandidate {
                name: "sdl12-compat".into(),
                repo: Some("extra".into()),
                version: None,
            },
        ];
        match a.answer_provider("sdl", &candidates) {
            ProviderDecision::Choose(i) => assert_eq!(
                i, 1,
                "must match by (name, repo) pair, not name alone — name-only would yield 0"
            ),
            other => panic!("expected Choose(1), got {other:?}"),
        }
    }
}
