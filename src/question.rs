use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Conflict {
    pub incoming: String,
    pub removable: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestionSet {
    pub conflicts: Vec<Conflict>,
    pub had_unsupported_question: bool,
    pub unsupported_summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Approvals {
    pub approved_conflicts: Vec<Conflict>,
}

impl QuestionSet {
    pub fn approve(&self, selected: &[usize]) -> Approvals {
        let approved_conflicts = selected
            .iter()
            .map(|&i| self.conflicts[i].clone())
            .collect();
        Approvals { approved_conflicts }
    }
}

pub fn encode_approvals(approvals: &Approvals) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(approvals).context("failed to serialize approvals")?;
    Ok(STANDARD.encode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_approvals_round_trips() {
        let qs = QuestionSet {
            conflicts: vec![Conflict {
                incoming: "cava-git".into(),
                removable: "cava".into(),
            }],
            had_unsupported_question: false,
            unsupported_summary: String::new(),
        };
        let approvals = qs.approve(&[0]);
        let b64 = encode_approvals(&approvals).expect("encode");
        let bytes = STANDARD.decode(&b64).expect("decode");
        let decoded: Approvals = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(decoded.approved_conflicts, approvals.approved_conflicts);
    }
}
