use crate::events::DiscardSink;
use crate::question::source::{AnswerSource, ExploreDefaults};
use crate::tx::driver::{RunOutcome, RunSpec, run};

pub fn preview(handle: &mut alpm::Alpm, spec: RunSpec) -> anyhow::Result<RunOutcome> {
    preview_with(handle, spec, Box::new(ExploreDefaults))
}

pub(crate) fn preview_with(
    handle: &mut alpm::Alpm,
    mut spec: RunSpec,
    source: Box<dyn AnswerSource>,
) -> anyhow::Result<RunOutcome> {
    spec.explore = true;
    run(handle, &spec, source, Box::new(DiscardSink))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::model::{Answer, Question};
    use crate::question::source::SourceDecision;
    use crate::tx::driver::Finish;
    use crate::tx::fixtures::{Pkg, summary_names};

    fn spec(targets: &[&str]) -> RunSpec {
        crate::tx::fixtures::sync_spec(targets, true)
    }

    #[test]
    fn preview_explores_with_explore_defaults_and_returns_review() {
        use crate::tx::fixtures::Pkg;

        let (_dir, mut handle) = crate::tx::fixtures::fixture(&[Pkg::plain("solo")]);
        let outcome = preview(&mut handle, spec(&["solo"])).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let review = outcome.review.expect("explore run carries a review");
        assert_eq!(
            review
                .part2
                .packages
                .iter()
                .map(|pkg| pkg.name.clone())
                .collect::<Vec<_>>(),
            vec!["solo".to_string()]
        );
        assert!(review.part1.is_empty());
        assert_eq!(review.generated_by.dbs.get("core").unwrap().packages, 1);
        assert!(handle.localdb().pkg("solo").is_err());
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }

    struct SecondProvider;

    impl AnswerSource for SecondProvider {
        fn answer(&self, question: &Question) -> SourceDecision {
            let Question::SelectProvider { candidates, .. } = question else {
                return ExploreDefaults.answer(question);
            };
            let Some(second) = candidates.get(1) else {
                return ExploreDefaults.answer(question);
            };
            SourceDecision::Answer(Answer::SelectProvider {
                name: second.name.clone(),
                repo: second.repo.clone(),
            })
        }
    }

    fn provider_fixture() -> (tempfile::TempDir, alpm::Alpm) {
        crate::tx::fixtures::fixture_full(
            &[(
                "core",
                vec![
                    Pkg {
                        provides: vec!["virt"],
                        ..Pkg::plain("provider-one")
                    },
                    Pkg {
                        provides: vec!["virt"],
                        ..Pkg::plain("provider-two")
                    },
                ],
            )],
            &[],
        )
    }

    #[test]
    fn preview_with_honors_second_provider_source() {
        let (_dir, mut handle) = provider_fixture();
        let defaults =
            preview_with(&mut handle, spec(&["virt"]), Box::new(ExploreDefaults)).unwrap();
        assert_eq!(summary_names(&defaults), vec!["provider-one".to_string()]);
        let second = preview_with(&mut handle, spec(&["virt"]), Box::new(SecondProvider)).unwrap();
        assert_eq!(summary_names(&second), vec!["provider-two".to_string()]);
    }
}
