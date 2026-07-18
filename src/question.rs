use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Conflict {
    pub incoming: String,
    pub removable: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
