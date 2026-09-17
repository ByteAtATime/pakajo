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
}

impl TerminalDecider {
    pub fn new(json: bool, skip_review: bool) -> Self {
        Self { json, skip_review }
    }
}

impl Decider for TerminalDecider {
    fn confirm_build(&self, plan: &Plan) -> BuildDecision {
        if self.json {
            return BuildDecision::Review;
        }
        if self.skip_review {
            crate::cli::prompts::confirm_build(plan)
        } else {
            crate::cli::prompts::confirm_proceed_to_review(plan)
        }
    }

    fn confirm_conflicts(&self, report: &ConflictReport) -> bool {
        if self.json {
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

pub struct AutomaticDecider;

impl Decider for AutomaticDecider {
    fn confirm_build(&self, _plan: &Plan) -> BuildDecision {
        BuildDecision::Proceed
    }

    fn confirm_conflicts(&self, _report: &ConflictReport) -> bool {
        false
    }

    fn review_pkgbuilds(&self, _pkgbuilds: &[PkgbuildInfo]) -> bool {
        true
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
    fn deterministic_json_and_automatic_deciders_answer_both_prompts() {
        use crate::build::BuildDecision;
        let plan = empty_plan();
        let terminal = TerminalDecider::new(true, false);
        let automatic = AutomaticDecider;
        let cases: Vec<(&dyn Decider, BuildDecision, bool)> = vec![
            (&terminal, BuildDecision::Review, true),
            (&automatic, BuildDecision::Proceed, true),
        ];
        for (decider, expected_confirm, expected_review) in cases {
            assert_eq!(decider.confirm_build(&plan), expected_confirm);
            assert_eq!(decider.review_pkgbuilds(&[]), expected_review);
        }
    }

    #[test]
    fn automatic_decider_bails_on_conflicts() {
        let report = conflicted_report();
        assert!(!AutomaticDecider.confirm_conflicts(&report));
    }

    #[test]
    fn terminal_json_decider_bails_on_conflicts_like_noconfirm() {
        let report = conflicted_report();
        let terminal = TerminalDecider::new(true, false);
        assert!(!terminal.confirm_conflicts(&report));
    }
}
