use std::sync::mpsc::{Receiver, Sender};

use crate::dispatch::install::{InstallRequest, run_install_preview, run_install_preview_with};
use crate::dispatch::remove::run_remove_preview;
use crate::events::TransactionSummary;
use crate::question::approvals::SealedApprovals;
use crate::question::model::{Answer, Question};
use crate::question::source::{ExploreDefaults, RevalidateSource, derive_answers};

pub enum ReviewPlan {
    Install(Box<InstallRequest>),
    Remove {
        targets: Vec<String>,
        holds: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum ReviewStep {
    Defaults,
    Sealed(SealedApprovals),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOrigin {
    Initial,
    Revalidation,
}

#[derive(Debug, Clone)]
pub struct RevalidationRun {
    pub origin: ReviewOrigin,
    pub questions: Vec<Question>,
    pub answers: Vec<Answer>,
    pub summary: TransactionSummary,
}

pub fn run_step(
    handle: &mut alpm::Alpm,
    plan: &ReviewPlan,
    step: ReviewStep,
) -> anyhow::Result<RevalidationRun> {
    match plan {
        ReviewPlan::Install(request) => match step {
            ReviewStep::Defaults => {
                let preview = run_install_preview(handle, request)?;
                let answers = derive_answers(&preview.review.part1, &ExploreDefaults)?;
                Ok(RevalidationRun {
                    origin: ReviewOrigin::Initial,
                    questions: preview.review.part1,
                    answers,
                    summary: preview.review.part2,
                })
            }
            ReviewStep::Sealed(sealed) => {
                let source = RevalidateSource::new(sealed);
                let preview = run_install_preview_with(handle, request, Box::new(source.clone()))?;
                let answers = derive_answers(&preview.review.part1, &source)?;
                Ok(RevalidationRun {
                    origin: ReviewOrigin::Revalidation,
                    questions: preview.review.part1,
                    answers,
                    summary: preview.review.part2,
                })
            }
        },
        ReviewPlan::Remove { targets, holds } => match step {
            ReviewStep::Defaults => {
                let review = run_remove_preview(handle, targets, holds, Box::new(ExploreDefaults))?;
                let answers = derive_answers(&review.part1, &ExploreDefaults)?;
                Ok(RevalidationRun {
                    origin: ReviewOrigin::Initial,
                    questions: review.part1,
                    answers,
                    summary: review.part2,
                })
            }
            ReviewStep::Sealed(sealed) => {
                let source = crate::question::source::ApprovalsReplay::new(sealed.clone());
                let review = run_remove_preview(handle, targets, holds, Box::new(source))?;
                let source = crate::question::source::ApprovalsReplay::new(sealed);
                let answers = derive_answers(&review.part1, &source)?;
                Ok(RevalidationRun {
                    origin: ReviewOrigin::Revalidation,
                    questions: review.part1,
                    answers,
                    summary: review.part2,
                })
            }
        },
    }
}

pub struct ReviewLoop {
    steps: Sender<ReviewStep>,
}

impl ReviewLoop {
    pub fn spawn(plan: ReviewPlan) -> (Self, Receiver<Result<RevalidationRun, String>>) {
        let (step_tx, step_rx) = std::sync::mpsc::channel::<ReviewStep>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<Result<RevalidationRun, String>>();
        std::thread::Builder::new()
            .name("review-loop".to_string())
            .spawn(move || drive_reviews(plan, step_rx, result_tx))
            .expect("spawn review-loop thread");
        (Self { steps: step_tx }, result_rx)
    }

    pub fn send(&self, step: ReviewStep) -> bool {
        if self.steps.send(step).is_err() {
            eprintln!("[pakajo] review loop gone; step dropped");
            return false;
        }
        true
    }
}

fn drive_reviews(
    plan: ReviewPlan,
    steps: Receiver<ReviewStep>,
    results: Sender<Result<RevalidationRun, String>>,
) {
    match prime_handle(&plan) {
        Ok(mut handle) => serve_reviews(&plan, &mut handle, steps, results),
        Err(message) => fail_closed(steps, results, message),
    }
}

fn prime_handle(plan: &ReviewPlan) -> Result<alpm::Alpm, String> {
    let config = crate::pacman::config().map_err(|error| format!("{error:#}"))?;
    let mut handle =
        crate::pacman::handle_with_config(&config).map_err(|error| format!("{error:#}"))?;
    if let ReviewPlan::Install(request) = plan {
        crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores);
    }
    Ok(handle)
}

fn serve_reviews(
    plan: &ReviewPlan,
    handle: &mut alpm::Alpm,
    steps: Receiver<ReviewStep>,
    results: Sender<Result<RevalidationRun, String>>,
) {
    while let Ok(step) = steps.recv() {
        let run = run_step(handle, plan, step).map_err(|error| format!("{error:#}"));
        if results.send(run).is_err() {
            break;
        }
    }
}

fn fail_closed(
    steps: Receiver<ReviewStep>,
    results: Sender<Result<RevalidationRun, String>>,
    message: String,
) {
    while steps.recv().is_ok() {
        if results.send(Err(message.clone())).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::protocol::AutomaticDecider;
    use crate::question::approvals::seal;

    fn sync_entry(
        builder: &mut tar::Builder<std::fs::File>,
        name: &str,
        version: &str,
        extra: &str,
    ) {
        let desc = format!(
            "%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}-{version}-x86_64.pkg.tar.zst\n\n{extra}"
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(desc.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{name}-{version}/desc"),
                desc.as_bytes(),
            )
            .unwrap();
    }

    fn open_handle(dir: &tempfile::TempDir, db: &std::path::Path) -> alpm::Alpm {
        let root = dir.path().join("root");
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        handle
            .register_syncdb_mut("core", alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
        handle
    }

    fn provider_handle() -> (tempfile::TempDir, alpm::Alpm, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        let stubs = dir.path().join("stubs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        std::fs::create_dir_all(&stubs).unwrap();
        let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        for name in ["provider-one", "provider-two"] {
            sync_entry(&mut builder, name, "1.0-1", "%PROVIDES%\nvirt\n\n");
        }
        builder.into_inner().unwrap();
        let handle = open_handle(&dir, &db);
        crate::tx::targets::write_cachedir_stub(
            &stubs,
            "needsvirt",
            "1.0-1",
            &crate::tx::targets::StubLists {
                depends: &["virt"],
                ..Default::default()
            },
        );
        let target = stubs
            .join(crate::tx::targets::filename("needsvirt", "1.0-1"))
            .to_string_lossy()
            .into_owned();
        (dir, handle, target)
    }

    fn plain_handle() -> (tempfile::TempDir, alpm::Alpm, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        sync_entry(&mut builder, "solo", "1.0-1", "");
        builder.into_inner().unwrap();
        let handle = open_handle(&dir, &db);
        let stub = crate::stub_pkg::build_stub_pkg("solo", "1.0-1", dir.path()).unwrap();
        let target = stub.to_string_lossy().into_owned();
        (dir, handle, target)
    }

    fn request(targets: &[&str]) -> InstallRequest {
        InstallRequest {
            targets: targets.iter().map(|target| target.to_string()).collect(),
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: Vec::new(),
            prefer_aur: false,
            decider: Box::new(AutomaticDecider::new()),
            approvals: None,
            tty: false,
            json: false,
        }
    }

    fn install_plan(targets: &[&str]) -> ReviewPlan {
        ReviewPlan::Install(Box::new(request(targets)))
    }

    fn remove_plan(targets: &[&str], holds: &[&str]) -> ReviewPlan {
        ReviewPlan::Remove {
            targets: targets.iter().map(|target| target.to_string()).collect(),
            holds: holds.iter().map(|hold| hold.to_string()).collect(),
        }
    }

    fn summary_names(run: &RevalidationRun) -> Vec<String> {
        let mut names: Vec<String> = run
            .summary
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect();
        names.sort();
        names
    }

    struct LocalEntry {
        name: &'static str,
        depends: &'static [&'static str],
        groups: &'static [&'static str],
    }

    fn local_entry(name: &'static str) -> LocalEntry {
        LocalEntry {
            name,
            depends: &[],
            groups: &[],
        }
    }

    fn local_handle(entries: &[LocalEntry]) -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        std::fs::create_dir_all(dir.path().join("cache")).unwrap();
        let handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        for entry in entries {
            let entry_dir = db.join("local").join(format!("{}-1.0-1", entry.name));
            std::fs::create_dir_all(&entry_dir).unwrap();
            let mut desc = format!(
                "%NAME%\n{}\n\n%VERSION%\n1.0-1\n\n%FILENAME%\n{}\n\n",
                entry.name,
                crate::tx::targets::filename(entry.name, "1.0-1"),
            );
            for (tag, values) in [("%DEPENDS%\n", entry.depends), ("%GROUPS%\n", entry.groups)] {
                if values.is_empty() {
                    continue;
                }
                desc.push_str(tag);
                for value in values {
                    desc.push_str(value);
                    desc.push('\n');
                }
                desc.push('\n');
            }
            std::fs::write(entry_dir.join("desc"), desc).unwrap();
            std::fs::write(entry_dir.join("files"), "%FILES%\n").unwrap();
        }
        (dir, handle)
    }

    fn sealed_from_defaults(run: &RevalidationRun) -> crate::question::approvals::SealedApprovals {
        let collectable: Vec<Question> = run
            .questions
            .iter()
            .filter(|question| question.is_collectable())
            .cloned()
            .collect();
        let answers = derive_answers(&collectable, &ExploreDefaults).expect("answers sealed");
        seal(&collectable, &answers, true).expect("seal succeeds")
    }

    #[test]
    fn defaults_step_surfaces_provider_question_with_summary() {
        let (_dir, mut handle, target) = provider_handle();
        let run = run_step(&mut handle, &install_plan(&[&target]), ReviewStep::Defaults).unwrap();
        assert_eq!(run.origin, ReviewOrigin::Initial);
        assert!(!run.questions.is_empty());
        assert!(
            run.questions.iter().any(|question| matches!(
                question,
                Question::SelectProvider { depend, .. } if depend == "virt"
            )),
            "expected a SelectProvider question for virt, got {:?}",
            run.questions
        );
        assert_eq!(run.questions.len(), run.answers.len());
        assert!(!run.summary.packages.is_empty());
        assert!(summary_names(&run).contains(&"needsvirt".to_string()));
    }

    #[test]
    fn same_handle_serves_second_step_with_equal_output() {
        let (_dir, mut handle, target) = provider_handle();
        let first = run_step(&mut handle, &install_plan(&[&target]), ReviewStep::Defaults).unwrap();
        let second =
            run_step(&mut handle, &install_plan(&[&target]), ReviewStep::Defaults).unwrap();
        assert_eq!(first.questions, second.questions);
        assert_eq!(first.answers, second.answers);
        assert_eq!(summary_names(&first), summary_names(&second));
    }

    #[test]
    fn plain_stub_projects_empty_questions_with_summary() {
        let (_dir, mut handle, target) = plain_handle();
        let run = run_step(&mut handle, &install_plan(&[&target]), ReviewStep::Defaults).unwrap();
        assert_eq!(run.origin, ReviewOrigin::Initial);
        assert!(run.questions.is_empty());
        assert!(run.answers.is_empty());
        assert_eq!(summary_names(&run), vec!["solo".to_string()]);
    }

    #[test]
    fn sealed_step_replays_provider_two_with_changed_summary() {
        let (_dir, mut handle, target) = provider_handle();
        let initial =
            run_step(&mut handle, &install_plan(&[&target]), ReviewStep::Defaults).unwrap();
        let reselected: Vec<Answer> = initial
            .answers
            .iter()
            .map(|answer| match answer {
                Answer::SelectProvider { .. } => Answer::SelectProvider {
                    name: "provider-two".to_string(),
                    repo: Some("core".to_string()),
                },
                kept => kept.clone(),
            })
            .collect();
        let sealed = seal(&initial.questions, &reselected, false).expect("seal succeeds");
        let rerun = run_step(
            &mut handle,
            &install_plan(&[&target]),
            ReviewStep::Sealed(sealed),
        )
        .unwrap();
        assert_eq!(rerun.origin, ReviewOrigin::Revalidation);
        assert!(
            rerun.questions.iter().any(|question| matches!(
                question,
                Question::SelectProvider { depend, .. } if depend == "virt"
            )),
            "expected a SelectProvider question for virt, got {:?}",
            rerun.questions
        );
        assert!(
            rerun.answers.iter().any(|answer| matches!(
                answer,
                Answer::SelectProvider { name, .. } if name == "provider-two"
            )),
            "expected provider-two replay, got {:?}",
            rerun.answers
        );
        assert_ne!(summary_names(&initial), summary_names(&rerun));
        assert!(summary_names(&rerun).contains(&"provider-two".to_string()));
        assert!(!summary_names(&rerun).contains(&"provider-one".to_string()));
    }

    #[test]
    fn remove_defaults_step_records_hold_question_with_removal_summary() {
        let app = LocalEntry {
            name: "app",
            depends: &["lib"],
            groups: &[],
        };
        let (_dir, mut handle) = local_handle(&[app, local_entry("lib")]);
        let plan = remove_plan(&["app"], &["app"]);
        let run = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert_eq!(run.origin, ReviewOrigin::Initial);
        assert!(
            run.questions.iter().any(|question| matches!(
                question,
                Question::HoldPkgs { names } if names == &vec!["app".to_string()]
            )),
            "expected a HoldPkgs question for app, got {:?}",
            run.questions
        );
        assert_eq!(run.questions.len(), run.answers.len());
        assert!(summary_names(&run).contains(&"app".to_string()));
        assert!(
            run.summary
                .packages
                .iter()
                .all(|package| package.is_removal),
            "expected every summary entry to be a removal, got {:?}",
            run.summary.packages
        );
    }

    #[test]
    fn remove_missing_target_records_removepkgs_question() {
        use crate::question::model::TransactionKind;

        let (_dir, mut handle) = local_handle(&[local_entry("sl")]);
        let plan = remove_plan(&["ghost", "sl"], &[]);
        let run = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert!(
            run.questions.iter().any(|question| matches!(
                question,
                Question::RemovePkgs { names, kind: TransactionKind::Remove }
                if names == &vec!["ghost".to_string()]
            )),
            "expected a RemovePkgs question for ghost, got {:?}",
            run.questions
        );
        assert_eq!(run.questions.len(), run.answers.len());
        assert!(summary_names(&run).contains(&"sl".to_string()));
    }

    #[test]
    fn remove_group_target_queues_all_local_members() {
        let one = LocalEntry {
            name: "member-one",
            depends: &[],
            groups: &["tools"],
        };
        let two = LocalEntry {
            name: "member-two",
            depends: &[],
            groups: &["tools"],
        };
        let (_dir, mut handle) = local_handle(&[one, two]);
        let plan = remove_plan(&["tools"], &[]);
        let run = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert_eq!(
            summary_names(&run),
            vec!["member-one".to_string(), "member-two".to_string()]
        );
    }

    #[test]
    fn remove_local_prefixed_target_resolves() {
        let (_dir, mut handle) = local_handle(&[local_entry("sl")]);
        let plan = remove_plan(&["local/sl"], &[]);
        let run = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert_eq!(summary_names(&run), vec!["sl".to_string()]);
    }

    #[test]
    fn remove_sealed_step_without_hold_answer_fails_closed() {
        let app = LocalEntry {
            name: "app",
            depends: &["lib"],
            groups: &[],
        };
        let (_dir, mut handle) = local_handle(&[app, local_entry("lib")]);
        let plan = remove_plan(&["app"], &["app"]);
        let initial = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert!(
            initial.questions.iter().any(|question| matches!(
                question,
                Question::HoldPkgs { names } if names == &vec!["app".to_string()]
            )),
            "expected a HoldPkgs question for app, got {:?}",
            initial.questions
        );
        let unanswered = crate::question::approvals::SealedApprovals {
            answers: Vec::new(),
            proceed: true,
            deps: Vec::new(),
        };
        let rerun = run_step(&mut handle, &plan, ReviewStep::Sealed(unanswered));
        assert!(
            rerun.is_err(),
            "sealed remove without the HoldPkgs answer must fail closed, got {:?}",
            rerun.map(|run| run.answers)
        );
    }

    #[test]
    fn remove_sealed_step_replays_and_converges() {
        use crate::question::review::fingerprint;

        let (_dir, mut handle) = local_handle(&[local_entry("sl")]);
        let plan = remove_plan(&["sl"], &["sl"]);
        let initial = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        let sealed = sealed_from_defaults(&initial);
        let rerun = run_step(&mut handle, &plan, ReviewStep::Sealed(sealed)).unwrap();
        assert_eq!(rerun.origin, ReviewOrigin::Revalidation);
        assert_eq!(summary_names(&initial), summary_names(&rerun));
        assert_eq!(
            fingerprint(&initial.questions, &initial.answers, &initial.summary),
            fingerprint(&rerun.questions, &rerun.answers, &rerun.summary)
        );
    }

    #[test]
    fn remove_same_handle_serves_second_step_with_equal_output() {
        let (_dir, mut handle) = local_handle(&[local_entry("sl")]);
        let plan = remove_plan(&["sl"], &["sl"]);
        let first = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        let second = run_step(&mut handle, &plan, ReviewStep::Defaults).unwrap();
        assert_eq!(first.questions, second.questions);
        assert_eq!(first.answers, second.answers);
        assert_eq!(summary_names(&first), summary_names(&second));
    }
}
