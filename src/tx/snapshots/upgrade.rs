use super::{approved, installed_names, snapshot_settings, summary_package_lines};
use crate::dispatch::sysupgrade::upgrade_review;
use crate::question::model::{Answer, Question};
use crate::question::source::{AnswerSource, ExploreDefaults, SourceDecision};
use crate::tx::fixtures::{Pkg, fixture_full};

fn sync_pkg(name: &'static str, version: &'static str) -> Pkg {
    Pkg::make(name, version, &[], &[], &[])
}

fn local_pkg(name: &'static str, version: &'static str) -> Pkg {
    Pkg::make(name, version, &[], &[], &[])
}

fn question_kind(question: &Question) -> &'static str {
    match question {
        Question::Conflict { .. } => "Conflict",
        Question::SelectProvider { .. } => "SelectProvider",
        Question::Replace { .. } => "Replace",
        Question::InstallIgnorepkg { .. } => "InstallIgnorepkg",
        Question::RemovePkgs { .. } => "RemovePkgs",
        Question::HoldPkgs { .. } => "HoldPkgs",
        Question::Corrupted { .. } => "Corrupted",
        Question::ImportKey { .. } => "ImportKey",
        Question::Proceed { .. } => "Proceed",
        Question::GroupMembers { .. } => "GroupMembers",
    }
}

fn explore_default(question: &Question) -> String {
    match ExploreDefaults.answer(question) {
        SourceDecision::Answer(answer) => match answer {
            Answer::Conflict { remove, .. } => format!("remove={remove}"),
            Answer::SelectProvider { name, repo } => {
                format!("provider={} repo={}", name, repo.as_deref().unwrap_or("-"))
            }
            Answer::Replace { replace, .. } => format!("replace={replace}"),
            Answer::InstallIgnorepkg { install, .. } => format!("install={install}"),
            Answer::RemovePkgs { skip, .. } => format!("skip={skip}"),
            Answer::HoldPkgs { proceed, .. } => format!("proceed={proceed}"),
            Answer::Corrupted { remove, .. } => format!("remove={remove}"),
            Answer::ImportKey { import, .. } => format!("import={import}"),
            Answer::Proceed => "proceed".to_string(),
            Answer::Stop => "stop".to_string(),
            Answer::GroupMembers { selected } => format!("members={}", selected.join(",")),
        },
        SourceDecision::Abort(denied) => format!("abort:{}", denied.reason),
    }
}

fn render_question(question: &Question) -> String {
    format!(
        "question {} {:?} default={}",
        question_kind(question),
        question.key(),
        explore_default(question)
    )
}

fn run_upgrade_case(local: &[Pkg], sync: &[Pkg]) -> String {
    let (_dir, mut handle) = fixture_full(&[("core", sync.to_vec())], local);
    let assessed =
        upgrade_review(&mut handle, approved()).expect("upgrade explore review succeeds");
    let mut rendered = String::new();
    for question in &assessed.review.part1 {
        rendered.push_str(&render_question(question));
        rendered.push('\n');
    }
    rendered.push_str("summary:\n");
    for line in summary_package_lines(&assessed.review.part2) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered.push_str("installed:\n");
    for line in installed_names(&handle) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered
}

#[test]
fn upgrade_plain_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_plain_snapshot",
            run_upgrade_case(
                &[local_pkg("upgun", "1.0-1")],
                &[sync_pkg("upgun", "2.0-1")],
            )
        );
    });
}

#[test]
fn upgrade_replace_conflict_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_replace_conflict_snapshot",
            run_upgrade_case(
                &[
                    local_pkg("dummy-old", "1.0-1"),
                    local_pkg("clash-a", "1.0-1"),
                    local_pkg("clash-b", "0.5-1"),
                ],
                &[
                    Pkg {
                        replaces: vec!["dummy-old"],
                        ..sync_pkg("dummy-new", "2.0-1")
                    },
                    Pkg {
                        conflicts: vec!["clash-a"],
                        ..sync_pkg("clash-b", "1.0-1")
                    },
                ],
            )
        );
    });
}

#[test]
fn upgrade_provider_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_provider_snapshot",
            run_upgrade_case(
                &[local_pkg("needsvirt", "1.0-1")],
                &[
                    Pkg {
                        depends: vec!["virt"],
                        ..sync_pkg("needsvirt", "2.0-1")
                    },
                    Pkg {
                        provides: vec!["virt"],
                        ..sync_pkg("provider-one", "1.0-1")
                    },
                    Pkg {
                        provides: vec!["virt"],
                        ..sync_pkg("provider-two", "1.0-1")
                    },
                ],
            )
        );
    });
}

#[test]
fn upgrade_nothing_to_do_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_nothing_to_do_snapshot",
            run_upgrade_case(
                &[local_pkg("upgun", "1.0-1")],
                &[sync_pkg("upgun", "1.0-1")],
            )
        );
    });
}
