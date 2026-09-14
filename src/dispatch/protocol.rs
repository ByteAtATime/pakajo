use crate::build::BuildDecision;
use crate::pkgbuild::PkgbuildInfo;
use crate::resolve::BuildPlan;

pub trait Decider {
    fn confirm_build(&self, plan: &BuildPlan) -> BuildDecision;
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
    fn confirm_build(&self, plan: &BuildPlan) -> BuildDecision {
        if self.json {
            return BuildDecision::Review;
        }
        if self.skip_review {
            crate::cli::prompts::confirm_build(plan)
        } else {
            crate::cli::prompts::confirm_proceed_to_review(plan)
        }
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
    fn confirm_build(&self, _plan: &BuildPlan) -> BuildDecision {
        BuildDecision::Proceed
    }

    fn review_pkgbuilds(&self, _pkgbuilds: &[PkgbuildInfo]) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{BuildLayer, BuildPlan};

    fn empty_plan() -> BuildPlan {
        BuildPlan {
            targets: vec!["cava-git".to_string()],
            layers: vec![BuildLayer {
                aur: vec![],
                repo_deps: vec![],
            }],
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
}
