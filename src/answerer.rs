use std::io::Write as _;

pub trait QuestionAnswerer {
    fn answer_conflict(&self, incoming: &str, removable: &str) -> ConflictDecision;
}

pub enum ConflictDecision {
    Remove,
    Decline,
    CannotPrompt,
}

pub struct StdioAnswerer;

impl StdioAnswerer {
    pub fn new() -> Self {
        Self
    }
}

impl QuestionAnswerer for StdioAnswerer {
    fn answer_conflict(&self, incoming: &str, removable: &str) -> ConflictDecision {
        eprint!(
            ":: {} and {} are in conflict. Remove {}? [y/N] ",
            incoming, removable, removable
        );
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
}

pub struct NonInteractiveAnswerer;

impl QuestionAnswerer for NonInteractiveAnswerer {
    fn answer_conflict(&self, _incoming: &str, _removable: &str) -> ConflictDecision {
        ConflictDecision::CannotPrompt
    }
}

pub struct DenyAllAnswerer;

impl QuestionAnswerer for DenyAllAnswerer {
    fn answer_conflict(&self, _incoming: &str, _removable: &str) -> ConflictDecision {
        ConflictDecision::Decline
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
    fn answer_conflict(&self, incoming: &str, removable: &str) -> ConflictDecision {
        let approved = self
            .approvals
            .approved_conflicts
            .iter()
            .any(|c| {
                (c.incoming == incoming && c.removable == removable)
                    || (c.incoming == removable && c.removable == incoming)
            });
        if approved {
            ConflictDecision::Remove
        } else {
            ConflictDecision::Decline
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{Approvals, Conflict};

    #[test]
    fn approvals_answerer_matches_in_either_direction() {
        let approvals = Approvals {
            approved_conflicts: vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }],
        };
        let a = ApprovalsAnswerer::new(approvals);
        assert!(matches!(
            a.answer_conflict("cava-git", "cava"),
            ConflictDecision::Remove
        ));
        assert!(matches!(
            a.answer_conflict("cava", "cava-git"),
            ConflictDecision::Remove
        ));
        assert!(matches!(
            a.answer_conflict("foo", "bar"),
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
        };
        let json = serde_json::to_vec(&original).expect("serialize");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&json);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("decode");
        let decoded: Approvals = serde_json::from_slice(&bytes).expect("deserialize");
        let a = ApprovalsAnswerer::new(decoded);
        assert!(matches!(
            a.answer_conflict("cava-git", "cava"),
            ConflictDecision::Remove
        ));
    }
}
