use serde::{Deserialize, Serialize};

use crate::events::MergeOrigin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeKind {
    Pacnew,
    Pacsave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeAction {
    KeepCurrent,
    TakeNew,
    Merge { hunks: Vec<usize> },
    Restore,
    Delete,
    Defer,
}

impl MergeOrigin {
    pub fn of(old: Option<&alpm::Package>, new: Option<&alpm::Package>) -> Option<MergeOrigin> {
        let package = new.or(old)?.name().to_string();
        Some(MergeOrigin {
            package,
            old_version: old.map(|p| p.version().to_string()),
            new_version: new.map(|p| p.version().to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeDecision {
    pub file: String,
    pub action: MergeAction,
}

impl MergeDecision {
    pub fn defer(file: &str) -> MergeDecision {
        MergeDecision {
            file: file.to_string(),
            action: MergeAction::Defer,
        }
    }

    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("merge decision serializes");
        line.push('\n');
        line
    }
}

fn has_duplicate(hunks: &[usize]) -> bool {
    let mut seen = std::collections::HashSet::new();
    hunks.iter().any(|hunk| !seen.insert(hunk))
}

fn normalize_to_defer(action: MergeAction) -> MergeAction {
    let MergeAction::Merge { hunks } = &action else {
        return action;
    };
    if hunks.is_empty() || has_duplicate(hunks) {
        return MergeAction::Defer;
    }
    action
}

pub fn parse_decision_line(line: &str, offered_file: &str) -> MergeAction {
    let Ok(decision) = serde_json::from_str::<MergeDecision>(line) else {
        return MergeAction::Defer;
    };
    if decision.file != offered_file {
        return MergeAction::Defer;
    }
    normalize_to_defer(decision.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_origin_of_nothing_is_nothing() {
        assert_eq!(MergeOrigin::of(None, None), None);
    }

    #[test]
    fn defer_decision_line_is_json_plus_newline() {
        assert_eq!(
            MergeDecision::defer("/etc/x.conf").to_line(),
            "{\"file\":\"/etc/x.conf\",\"action\":\"Defer\"}\n"
        );
    }

    #[test]
    fn keep_current_for_the_offered_file_is_honored() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/x.conf","action":"KeepCurrent"}"#,
                "/etc/x.conf"
            ),
            MergeAction::KeepCurrent
        );
    }

    #[test]
    fn take_new_for_the_offered_file_is_honored() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/x.conf","action":"TakeNew"}"#,
                "/etc/x.conf"
            ),
            MergeAction::TakeNew
        );
    }

    #[test]
    fn decision_for_a_different_file_defers() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/other.conf","action":"KeepCurrent"}"#,
                "/etc/x.conf"
            ),
            MergeAction::Defer
        );
    }

    #[test]
    fn malformed_decision_line_defers() {
        assert_eq!(
            parse_decision_line("not json at all", "/etc/x.conf"),
            MergeAction::Defer
        );
    }

    #[test]
    fn empty_decision_line_defers() {
        assert_eq!(parse_decision_line("", "/etc/x.conf"), MergeAction::Defer);
    }

    #[test]
    fn merge_with_duplicate_hunks_defers() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/x.conf","action":{"Merge":{"hunks":[0,0]}}}"#,
                "/etc/x.conf"
            ),
            MergeAction::Defer
        );
    }

    #[test]
    fn merge_with_empty_hunks_defers() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/x.conf","action":{"Merge":{"hunks":[]}}}"#,
                "/etc/x.conf"
            ),
            MergeAction::Defer
        );
    }

    #[test]
    fn merge_preserves_out_of_order_hunks() {
        assert_eq!(
            parse_decision_line(
                r#"{"file":"/etc/x.conf","action":{"Merge":{"hunks":[2,0]}}}"#,
                "/etc/x.conf"
            ),
            MergeAction::Merge { hunks: vec![2, 0] }
        );
    }
}
