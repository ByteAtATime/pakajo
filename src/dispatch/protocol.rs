use crate::build::BuildDecision;
use crate::pkgbuild::PkgbuildInfo;
use crate::resolve::{ConflictReport, Plan};

pub trait Decider {
    fn confirm_build(&self, plan: &Plan) -> BuildDecision;
    fn confirm_conflicts(&self, report: &ConflictReport) -> bool;
    fn review_pkgbuilds(&self, pkgbuilds: &[PkgbuildInfo]) -> bool;
}

pub struct TerminalDecider {
    json: bool,
    skip_review: bool,
    tty: bool,
}

impl TerminalDecider {
    pub fn new(json: bool, skip_review: bool, tty: bool) -> Self {
        Self {
            json,
            skip_review,
            tty,
        }
    }
}

impl Decider for TerminalDecider {
    fn confirm_build(&self, plan: &Plan) -> BuildDecision {
        if self.json {
            return BuildDecision::Review;
        }
        let go_to_review = !plan.bases.is_empty() && !self.skip_review;
        if go_to_review {
            crate::cli::prompts::confirm_proceed_to_review(plan)
        } else {
            crate::cli::prompts::confirm_proceed_install(plan)
        }
    }

    fn confirm_conflicts(&self, report: &ConflictReport) -> bool {
        if self.json || !self.tty {
            return false;
        }
        crate::cli::prompts::confirm_conflicts(report)
    }

    fn review_pkgbuilds(&self, pkgbuilds: &[PkgbuildInfo]) -> bool {
        if self.json {
            return true;
        }
        crate::cli::review::review_pkgbuilds(pkgbuilds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Conflict, ConflictReport, Conflicting, Plan};

    fn empty_plan() -> Plan {
        Plan {
            bases: Vec::new(),
            repo_installs: Vec::new(),
            missing: Vec::new(),
            conflicts: ConflictReport {
                local: Vec::new(),
                inner: Vec::new(),
            },
            duplicates: Vec::new(),
        }
    }

    fn conflicted_report() -> ConflictReport {
        ConflictReport {
            local: vec![Conflict {
                pkg: "cava".to_string(),
                conflicting: vec![Conflicting {
                    pkg: "cava-git".to_string(),
                    conflict: Some("cava".to_string()),
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn deterministic_json_decider_answers_both_prompts() {
        use crate::build::BuildDecision;
        let plan = empty_plan();
        let terminal = TerminalDecider::new(true, false, false);
        assert_eq!(terminal.confirm_build(&plan), BuildDecision::Review);
        assert!(terminal.review_pkgbuilds(&[]));
    }

    #[test]
    fn terminal_json_decider_bails_on_conflicts_like_noconfirm() {
        let report = conflicted_report();
        let terminal = TerminalDecider::new(true, false, false);
        assert!(!terminal.confirm_conflicts(&report));
    }

    #[test]
    fn terminal_headless_decider_bails_on_conflicts_without_prompting() {
        let report = conflicted_report();
        let terminal = TerminalDecider::new(false, false, false);
        assert!(!terminal.confirm_conflicts(&report));
    }
}
