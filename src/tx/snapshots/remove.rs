use super::{approved, render_outcome, snapshot_settings};
use crate::install::{OfflinePkg, drive_sync, offline_pkg, offline_root};
use crate::question::approvals::seal;
use crate::question::model::{Answer, Question};
use crate::question::source::{
    AnswerSource, ApprovalsReplay, ExploreDefaults, SourceDecision, derive_answers,
};
use crate::tx::driver::{RemoveSpec, RunKind, RunSpec};

enum RemoveSource {
    Hold(bool),
    Missing(bool),
    SealedHold,
    Proceed,
}

struct RemoveCase {
    name: &'static str,
    packages: Vec<OfflinePkg>,
    preinstall: Vec<&'static str>,
    holds: Vec<&'static str>,
    targets: Vec<&'static str>,
    source: RemoveSource,
}

struct Discard;

impl crate::events::InstallSink for Discard {
    fn event(&mut self, _event: crate::events::InstallEvent) {}
}

fn remove_cases() -> Vec<RemoveCase> {
    let member_one = OfflinePkg {
        name: "member-one",
        groups: &["tools"],
        ..offline_pkg("member-one")
    };
    let member_two = OfflinePkg {
        name: "member-two",
        groups: &["tools"],
        ..offline_pkg("member-two")
    };
    vec![
        RemoveCase {
            name: "hold accept",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: vec!["sl"],
            targets: vec!["sl"],
            source: RemoveSource::Hold(true),
        },
        RemoveCase {
            name: "hold decline",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: vec!["sl"],
            targets: vec!["sl"],
            source: RemoveSource::Hold(false),
        },
        RemoveCase {
            name: "missing skip",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: Vec::new(),
            targets: vec!["sl", "ghost"],
            source: RemoveSource::Missing(true),
        },
        RemoveCase {
            name: "missing abort",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: Vec::new(),
            targets: vec!["sl", "ghost"],
            source: RemoveSource::Missing(false),
        },
        RemoveCase {
            name: "sealed replay",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: vec!["sl"],
            targets: vec!["sl"],
            source: RemoveSource::SealedHold,
        },
        RemoveCase {
            name: "group queue-all",
            packages: vec![member_one, member_two],
            preinstall: vec!["member-one", "member-two"],
            holds: Vec::new(),
            targets: vec!["tools"],
            source: RemoveSource::Proceed,
        },
        RemoveCase {
            name: "local prefixed target",
            packages: vec![offline_pkg("sl")],
            preinstall: vec!["sl"],
            holds: Vec::new(),
            targets: vec!["local/sl"],
            source: RemoveSource::Proceed,
        },
    ]
}

fn scripted_source(hold: bool, skip: bool) -> Box<dyn AnswerSource> {
    struct Script {
        hold: bool,
        skip: bool,
    }
    impl AnswerSource for Script {
        fn answer(&self, question: &Question) -> SourceDecision {
            match question {
                Question::HoldPkgs { names } => SourceDecision::Answer(Answer::HoldPkgs {
                    names: names.clone(),
                    proceed: self.hold,
                }),
                Question::RemovePkgs { names, .. } => SourceDecision::Answer(Answer::RemovePkgs {
                    names: names.clone(),
                    skip: self.skip,
                }),
                Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
                _ => SourceDecision::Answer(Answer::Stop),
            }
        }
    }
    Box::new(Script { hold, skip })
}

fn spec_for(case: &RemoveCase, explore: bool) -> RunSpec {
    RunSpec {
        kind: RunKind::Remove(RemoveSpec {
            flags: alpm::TransFlag::NONE,
            holds: case.holds.iter().map(|hold| hold.to_string()).collect(),
        }),
        targets: case
            .targets
            .iter()
            .map(|target| target.to_string())
            .collect(),
        stub_targets: Vec::new(),
        explore,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    }
}

fn sealed_hold_source(handle: &mut alpm::Alpm, case: &RemoveCase) -> Box<dyn AnswerSource> {
    let preview = crate::tx::driver::run(
        handle,
        &spec_for(case, true),
        Box::new(ExploreDefaults),
        Box::new(Discard),
    )
    .expect("explore pass captures sealed questions");
    let review = preview.review.expect("explore run carries a review");
    let collectable: Vec<Question> = review
        .part1
        .into_iter()
        .filter(|question| question.is_collectable())
        .collect();
    let answers =
        derive_answers(&collectable, &ExploreDefaults).expect("explore pass answers sealed");
    let sealed = seal(&collectable, &answers, true).expect("captured answers seal");
    Box::new(ApprovalsReplay::new(sealed))
}

fn remove_source(source: &RemoveSource) -> Option<Box<dyn AnswerSource>> {
    match source {
        RemoveSource::Hold(proceed) => Some(scripted_source(*proceed, true)),
        RemoveSource::Missing(skip) => Some(scripted_source(true, *skip)),
        RemoveSource::Proceed => Some(scripted_source(true, true)),
        RemoveSource::SealedHold => None,
    }
}

fn run_remove_case(case: &RemoveCase) -> String {
    let (_dir, mut handle) = offline_root(&case.packages);
    for target in &case.preinstall {
        drive_sync(&mut handle, std::slice::from_ref(target), approved()).unwrap();
    }
    let source = match remove_source(&case.source) {
        Some(source) => source,
        None => sealed_hold_source(&mut handle, case),
    };
    let result = crate::tx::driver::run(
        &mut handle,
        &spec_for(case, false),
        source,
        Box::new(Discard),
    );
    render_outcome(&result, &handle)
}

#[test]
fn remove_behavior_snapshot() {
    snapshot_settings().bind(|| {
        let mut rendered = String::new();
        for (index, case) in remove_cases().iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&format!("== {} ==\n{}", case.name, run_remove_case(case)));
        }
        insta::assert_snapshot!("remove_behavior_snapshot", rendered);
    });
}
