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

#[derive(Default)]
pub struct AutomaticDecider {
    approvals: crate::question::Approvals,
}

impl AutomaticDecider {
    pub fn new() -> Self {
        Self {
            approvals: crate::question::Approvals::default(),
        }
    }

    pub fn with_approvals(approvals: crate::question::Approvals) -> Self {
        Self { approvals }
    }

    fn conflict_approved(&self, incoming: &str, removable: &str) -> bool {
        self.approvals.approved_conflicts.iter().any(|approved| {
            (approved.incoming == incoming && approved.removable == removable)
                || (approved.incoming == removable && approved.removable == incoming)
        })
    }
}

impl Decider for AutomaticDecider {
    fn confirm_build(&self, _plan: &Plan) -> BuildDecision {
        BuildDecision::Proceed
    }

    fn confirm_conflicts(&self, report: &ConflictReport) -> bool {
        report
            .local
            .iter()
            .chain(report.inner.iter())
            .flat_map(|conflict| {
                conflict
                    .conflicting
                    .iter()
                    .map(|entry| (conflict.pkg.as_str(), entry.pkg.as_str()))
            })
            .all(|(incoming, removable)| self.conflict_approved(incoming, removable))
            && !report.is_empty()
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
        let terminal = TerminalDecider::new(true, false, false);
        let automatic = AutomaticDecider::new();
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
        assert!(!AutomaticDecider::new().confirm_conflicts(&report));
    }

    #[test]
    fn automatic_decider_proceeds_when_every_pair_approved() {
        let report = conflicted_report();
        let approvals = crate::question::Approvals {
            approved_conflicts: vec![crate::question::Conflict {
                incoming: "cava-git".to_string(),
                removable: "cava".to_string(),
            }],
            approved_providers: Vec::new(),
            approved_held: Vec::new(),
            approved_groups: Default::default(),
        };
        assert!(AutomaticDecider::with_approvals(approvals).confirm_conflicts(&report));
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
