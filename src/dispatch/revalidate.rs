use std::sync::mpsc::{Receiver, Sender};

use crate::dispatch::install::{InstallRequest, run_install_preview};
use crate::events::TransactionSummary;
use crate::question::model::Question;

#[derive(Debug, Clone)]
pub enum ReviewStep {
    Defaults,
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
    pub summary: TransactionSummary,
}

pub fn run_job(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
    step: ReviewStep,
) -> anyhow::Result<RevalidationRun> {
    let ReviewStep::Defaults = step;
    let preview = run_install_preview(handle, request)?;
    Ok(RevalidationRun {
        origin: ReviewOrigin::Initial,
        questions: preview.review.part1,
        summary: preview.review.part2,
    })
}

pub struct ReviewLoop {
    steps: Sender<ReviewStep>,
}

impl ReviewLoop {
    pub fn spawn(request: InstallRequest) -> (Self, Receiver<Result<RevalidationRun, String>>) {
        let (step_tx, step_rx) = std::sync::mpsc::channel::<ReviewStep>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<Result<RevalidationRun, String>>();
        std::thread::Builder::new()
            .name("review-loop".to_string())
            .spawn(move || review_worker(request, step_rx, result_tx))
            .expect("spawn review-loop worker");
        (Self { steps: step_tx }, result_rx)
    }

    pub fn send(&self, step: ReviewStep) {
        let _ = self.steps.send(step);
    }
}

fn review_worker(
    request: InstallRequest,
    steps: Receiver<ReviewStep>,
    results: Sender<Result<RevalidationRun, String>>,
) {
    match prime_handle(&request) {
        Ok(mut handle) => serve_reviews(&request, &mut handle, steps, results),
        Err(message) => fail_closed(steps, results, message),
    }
}

fn prime_handle(request: &InstallRequest) -> Result<alpm::Alpm, String> {
    let config = crate::pacman::config().map_err(|error| format!("{error:#}"))?;
    let mut handle =
        crate::pacman::handle_with_config(&config).map_err(|error| format!("{error:#}"))?;
    crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores);
    Ok(handle)
}

fn serve_reviews(
    request: &InstallRequest,
    handle: &mut alpm::Alpm,
    steps: Receiver<ReviewStep>,
    results: Sender<Result<RevalidationRun, String>>,
) {
    while let Ok(step) = steps.recv() {
        let run = run_job(handle, request, step).map_err(|error| format!("{error:#}"));
        let _ = handle.trans_release();
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
            &["virt"],
            &[],
            &[],
            &[],
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

    #[test]
    fn defaults_job_surfaces_provider_question_with_summary() {
        let (_dir, mut handle, target) = provider_handle();
        let run = run_job(&mut handle, &request(&[&target]), ReviewStep::Defaults).unwrap();
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
        assert!(!run.summary.packages.is_empty());
        assert!(summary_names(&run).contains(&"needsvirt".to_string()));
    }

    #[test]
    fn same_handle_serves_second_job_with_equal_output() {
        let (_dir, mut handle, target) = provider_handle();
        let first = run_job(&mut handle, &request(&[&target]), ReviewStep::Defaults).unwrap();
        let second = run_job(&mut handle, &request(&[&target]), ReviewStep::Defaults).unwrap();
        assert_eq!(first.questions, second.questions);
        assert_eq!(summary_names(&first), summary_names(&second));
    }

    #[test]
    fn plain_stub_projects_empty_questions_with_summary() {
        let (_dir, mut handle, target) = plain_handle();
        let run = run_job(&mut handle, &request(&[&target]), ReviewStep::Defaults).unwrap();
        assert_eq!(run.origin, ReviewOrigin::Initial);
        assert!(run.questions.is_empty());
        assert_eq!(summary_names(&run), vec!["solo".to_string()]);
    }
}
