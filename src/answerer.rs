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
