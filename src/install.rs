use std::cell::RefCell;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::rc::Rc;

use anyhow::{Context, anyhow};
use futures::channel::mpsc;

use alpm::DownloadResult as AlpmDownloadResult;
use alpm::LogLevel as AlpmLogLevel;
use alpm::Progress as AlpmProgress;

use crate::events::{
    DownloadResult, InstallEvent, InstallSink, LogLevel, PackageOp, ProgressPhase, SummaryPackage,
    TransactionSummary,
};

#[cfg(test)]
pub(crate) use tests::setup_fake_root;

pub(crate) struct QuestionState {
    pub(crate) deny_flag: bool,
    pub(crate) detail: String,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
}

impl QuestionState {
    pub(crate) fn new(answerer: Box<dyn crate::answerer::QuestionAnswerer>) -> Self {
        Self {
            deny_flag: false,
            detail: String::new(),
            answerer,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum InstallProgress {
    Idle,
    Running,
    Failed(String),
    ConflictReview(crate::question::QuestionSet),
    Completed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub(crate) enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

pub(crate) enum StreamItem {
    Event(InstallEvent),
    Done(ChildOutcome),
}

pub enum InstallTarget {
    Repo(String),
    File(PathBuf),
}

pub fn run_install<S: InstallSink + 'static, F: FnOnce() -> bool>(
    targets: &[InstallTarget],
    as_deps: bool,
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm(&config)?;
    install_into(&mut handle, targets, as_deps, sink, confirm, answerer)
}

pub(crate) fn install_into<S: InstallSink + 'static, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[InstallTarget],
    as_deps: bool,
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    let qstate = Rc::new(RefCell::new(QuestionState::new(answerer)));
    register_callbacks(handle, sink.clone(), qstate.clone());
    let result = run_transaction(handle, targets, as_deps, &sink, &qstate, confirm);
    let _ = handle.trans_release();
    result
}

pub(crate) fn register_callbacks<S: InstallSink + 'static>(
    handle: &alpm::Alpm,
    sink: Rc<RefCell<S>>,
    qstate: Rc<RefCell<QuestionState>>,
) {
    handle.set_event_cb(sink.clone(), |any_event, data| {
        if let Some(event) = convert_event(any_event) {
            data.borrow_mut().event(event);
        }
    });

    handle.set_dl_cb(sink.clone(), |filename, any_ev, data| {
        if let Some(event) = convert_download(filename, any_ev) {
            data.borrow_mut().event(event);
        }
    });

    handle.set_progress_cb(
        sink.clone(),
        |phase, pkgname, percent, howmany, current, data| {
            let event = InstallEvent::Progress {
                phase: convert_progress_phase(phase),
                package: pkgname.to_string(),
                percent,
                current,
                total: howmany,
            };
            data.borrow_mut().event(event);
        },
    );

    handle.set_log_cb(sink, |level, message, data| {
        if let Some(mapped) = convert_log_level(level) {
            data.borrow_mut().event(InstallEvent::Log {
                level: mapped,
                message: message.to_string(),
            });
        }
    });

    handle.set_question_cb(
        qstate,
        |any_question: alpm::AnyQuestion, data: &mut Rc<RefCell<QuestionState>>| {
            let mut s = data.borrow_mut();
            match any_question.question() {
                alpm::Question::Conflict(mut cq) => {
                    let (incoming, incoming_version, removable, removable_version) = {
                        let c = cq.conflict();
                        (
                            c.package1().name().to_string(),
                            c.package1().version().to_string(),
                            c.package2().name().to_string(),
                            c.package2().version().to_string(),
                        )
                    };
                    match s.answerer.answer_conflict(
                        &incoming,
                        &incoming_version,
                        &removable,
                        &removable_version,
                    ) {
                        crate::answerer::ConflictDecision::Remove => {
                            cq.set_remove(true);
                        }
                        crate::answerer::ConflictDecision::Decline => {
                            s.deny_flag = true;
                            s.detail = format!("declined to remove {removable}");
                        }
                crate::answerer::ConflictDecision::CannotPrompt => {
                    s.deny_flag = true;
                    s.detail = format!(
                        "cannot prompt for conflict ({incoming} vs {removable}): stdin is not a terminal; re-run from an interactive shell"
                    );
                }
            }
        }
        alpm::Question::SelectProvider(mut spq) => {
            let depend = spq.depend().to_string();
            let candidates: Vec<crate::question::ProviderCandidate> = spq
                .providers()
                .into_iter()
                .map(|p| crate::question::ProviderCandidate {
                    name: p.name().to_string(),
                    repo: p.db().map(|d| d.name().to_string()),
                    version: Some(p.version().to_string()),
                })
                .collect();
            match s.answerer.answer_provider(&depend, &candidates) {
                crate::answerer::ProviderDecision::Choose(i) => spq.set_index(i as i32),
                crate::answerer::ProviderDecision::Decline => {
                    s.deny_flag = true;
                    s.detail = format!("declined to choose a provider for {depend}");
                }
                crate::answerer::ProviderDecision::CannotPrompt => {
                    s.deny_flag = true;
                    s.detail = format!(
                        "cannot prompt for provider ({depend}): stdin is not a terminal; re-run from an interactive shell"
                    );
                }
            }
        }
        alpm::Question::RemovePkgs(mut rq) => {
            rq.set_skip(false);
        }
        _ => {
            s.deny_flag = true;
            s.detail = "unsupported transaction question".to_string();
        }
            }
        },
    );
}

fn run_transaction<S: InstallSink, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[InstallTarget],
    as_deps: bool,
    sink: &Rc<RefCell<S>>,
    qstate: &Rc<RefCell<QuestionState>>,
    confirm: F,
) -> anyhow::Result<()> {
    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;

    let mut added_names: Vec<String> = Vec::with_capacity(targets.len());
    for target in targets {
        match target {
            InstallTarget::Repo(name) => {
                let pkg = crate::pacman::find_pkg(handle, name)
                    .ok_or_else(|| anyhow!("package '{name}' not found in any repository"))?;
                added_names.push(name.clone());
                handle
                    .trans_add_pkg(pkg)
                    .map_err(alpm::Error::from)
                    .context("failed to queue package for installation")?;
            }
            InstallTarget::File(path) => {
                let loaded = handle
                    .pkg_load(path.to_string_lossy().as_ref(), true, alpm::SigLevel::NONE)
                    .context("failed to load package file")?;
                added_names.push(loaded.name().to_string());
                handle
                    .trans_add_pkg(loaded)
                    .map_err(alpm::Error::from)
                    .context("failed to queue package file for installation")?;
            }
        }
    }

    let prepare_result = handle.trans_prepare();
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }
    prepare_result
        .map_err(alpm::Error::from)
        .context("failed to prepare transaction")?;

    let summary = build_summary(handle);
    sink.borrow_mut()
        .event(InstallEvent::TransactionSummary(summary));

    if !confirm() {
        return Ok(());
    }

    handle
        .trans_commit()
        .context("failed to commit transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    if as_deps {
        for name in &added_names {
            if let Ok(pkg) = handle.localdb().pkg(name.as_str()) {
                pkg.set_reason(alpm::PackageReason::Depend)
                    .context("failed to mark package as dependency")?;
            }
        }
    }

    Ok(())
}

pub(crate) fn build_summary(handle: &alpm::Alpm) -> TransactionSummary {
    let mut packages = Vec::new();
    let mut total_download_size = 0;
    let mut total_installed_size = 0;
    for pkg in handle.trans_add().iter() {
        let name = pkg.name().to_string();
        let old_version = handle
            .localdb()
            .pkg(name.as_str())
            .ok()
            .map(|p| p.version().to_string());
        let download_size = pkg.download_size();
        let installed_size = pkg.isize();
        total_download_size += download_size;
        total_installed_size += installed_size;
        packages.push(SummaryPackage {
            repository: pkg.db().map(|d| d.name().to_string()),
            new_version: pkg.version().to_string(),
            name,
            old_version,
            download_size,
            installed_size,
        });
    }
    TransactionSummary {
        packages,
        total_download_size,
        total_installed_size,
    }
}

fn convert_event(any_event: alpm::AnyEvent) -> Option<InstallEvent> {
    #[cfg(debug_assertions)]
    {
        let event_ptr: *const () = unsafe { std::mem::transmute_copy(&any_event) };
        if !(event_ptr as usize).is_multiple_of(std::mem::align_of::<*const ()>()) {
            return None;
        }
    }
    match any_event.event() {
        alpm::Event::ResolveDepsStart => Some(InstallEvent::ResolvingDependencies),
        alpm::Event::InterConflictsStart => Some(InstallEvent::CheckingConflicts),
        alpm::Event::FileConflictsStart => Some(InstallEvent::CheckingFileConflicts),
        alpm::Event::IntegrityStart => Some(InstallEvent::CheckingIntegrity),
        alpm::Event::DiskSpaceStart => Some(InstallEvent::CheckingDiskSpace),
        alpm::Event::LoadStart => Some(InstallEvent::LoadingPackages),
        alpm::Event::KeyringStart => Some(InstallEvent::KeyringStart),
        alpm::Event::PkgRetrieveStart(e) => Some(InstallEvent::RetrievingPackages {
            num: e.num(),
            total_bytes: e.total_size(),
        }),
        alpm::Event::PackageOperationStart(e) => {
            let (operation, package, new_version, old_version) =
                convert_package_operation(e.operation());
            Some(InstallEvent::PackageOperation {
                operation,
                package,
                new_version,
                old_version,
            })
        }
        alpm::Event::ScriptletInfo(e) => Some(InstallEvent::ScriptletInfo {
            line: e.line().to_string(),
        }),
        alpm::Event::HookRunStart(e) => Some(InstallEvent::HookRun {
            position: e.position(),
            total: e.total(),
            name: e.name().to_string(),
            desc: e.desc().map(str::to_string),
        }),
        alpm::Event::TransactionDone => Some(InstallEvent::TransactionDone),
        alpm::Event::TransactionStart => Some(InstallEvent::ProcessingChanges),
        _ => None,
    }
}

fn convert_package_operation(
    op: alpm::PackageOperation,
) -> (PackageOp, String, Option<String>, Option<String>) {
    let (operation, new, old): (PackageOp, Option<&alpm::Package>, Option<&alpm::Package>) =
        match op {
            alpm::PackageOperation::Install(p) => (PackageOp::Install, Some(p), None),
            alpm::PackageOperation::Upgrade(n, o) => (PackageOp::Upgrade, Some(n), Some(o)),
            alpm::PackageOperation::Reinstall(n, o) => (PackageOp::Reinstall, Some(n), Some(o)),
            alpm::PackageOperation::Downgrade(n, o) => (PackageOp::Downgrade, Some(n), Some(o)),
            alpm::PackageOperation::Remove(p) => (PackageOp::Remove, None, Some(p)),
        };
    let package = new
        .or(old)
        .map(|p| p.name().to_string())
        .unwrap_or_default();
    (
        operation,
        package,
        new.map(|p| p.version().to_string()),
        old.map(|p| p.version().to_string()),
    )
}

fn convert_download(filename: &str, any_ev: alpm::AnyDownloadEvent) -> Option<InstallEvent> {
    let filename = filename.to_string();
    Some(match any_ev.event() {
        alpm::DownloadEvent::Init(init) => InstallEvent::DownloadInit {
            filename,
            optional: init.optional,
        },
        alpm::DownloadEvent::Progress(p) => InstallEvent::DownloadProgress {
            filename,
            downloaded: p.downloaded,
            total: p.total,
        },
        alpm::DownloadEvent::Retry(r) => InstallEvent::DownloadRetry {
            filename,
            resume: r.resume,
        },
        alpm::DownloadEvent::Completed(c) => InstallEvent::DownloadCompleted {
            filename,
            total: c.total,
            result: convert_download_result(c.result),
        },
    })
}

fn convert_download_result(result: AlpmDownloadResult) -> DownloadResult {
    match result {
        AlpmDownloadResult::Success => DownloadResult::Success,
        AlpmDownloadResult::UpToDate => DownloadResult::UpToDate,
        AlpmDownloadResult::Failed => DownloadResult::Failed,
    }
}

fn convert_progress_phase(phase: AlpmProgress) -> ProgressPhase {
    match phase {
        AlpmProgress::AddStart => ProgressPhase::Add,
        AlpmProgress::UpgradeStart => ProgressPhase::Upgrade,
        AlpmProgress::DowngradeStart => ProgressPhase::Downgrade,
        AlpmProgress::ReinstallStart => ProgressPhase::Reinstall,
        AlpmProgress::RemoveStart => ProgressPhase::Remove,
        AlpmProgress::ConflictsStart => ProgressPhase::Conflicts,
        AlpmProgress::DiskspaceStart => ProgressPhase::Diskspace,
        AlpmProgress::IntegrityStart => ProgressPhase::Integrity,
        AlpmProgress::LoadStart => ProgressPhase::Load,
        AlpmProgress::KeyringStart => ProgressPhase::Keyring,
    }
}

fn convert_log_level(level: AlpmLogLevel) -> Option<LogLevel> {
    if level.contains(AlpmLogLevel::ERROR) {
        Some(LogLevel::Error)
    } else if level.contains(AlpmLogLevel::WARNING) {
        Some(LogLevel::Warning)
    } else {
        None
    }
}

pub(crate) fn run_install_process(
    exe: PathBuf,
    name: String,
    mut tx: mpsc::Sender<StreamItem>,
    approvals_b64: Option<String>,
) {
    let mut send_event = |mut item: StreamItem| loop {
        match tx.try_send(item) {
            Ok(()) => return,
            Err(err) => {
                if err.is_disconnected() {
                    return;
                }
                item = err.into_inner();
                std::thread::yield_now();
            }
        }
    };

    let mut cmd = Command::new(&exe);
    cmd.arg("install").arg("--json");
    if let Some(b64) = &approvals_b64 {
        cmd.arg("--approvals").arg(b64);
    }
    let outcome = match cmd
        .arg(&name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Err(error) if error.kind() == io::ErrorKind::NotFound => ChildOutcome::NotFound,
        Err(error) => ChildOutcome::Failed(error.to_string()),
        Ok(mut child) => {
            let stdout = child.stdout.take().expect("piped");
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if let Ok(ev) = serde_json::from_str::<InstallEvent>(&l) {
                            send_event(StreamItem::Event(ev));
                        }
                    }
                    Err(_) => break,
                }
            }
            map_outcome(child.wait())
        }
    };
    send_event(StreamItem::Done(outcome));
}

pub(crate) fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aur::AurInfo;
    use crate::cli::ConsoleSink;
    use crate::resolve::{BuildLayer, BuildPlan};
    use std::fs;

    pub(crate) fn setup_fake_root(suffix: &str) -> alpm::Alpm {
        let base = std::env::temp_dir().join(format!("pakajo_fake_root_{suffix}"));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&db).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let config = pacmanconf::Config::new().unwrap();
        let mut handle = crate::pacman::init_alpm_at(
            &config,
            &root.to_string_lossy(),
            &db.to_string_lossy(),
            &[cache.to_string_lossy().into_owned()],
        )
        .unwrap();
        handle.syncdbs_mut().update(false).unwrap();
        handle
    }

    #[test]
    #[ignore]
    fn test_install() {
        let mut handle = setup_fake_root("install");
        let result = install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        result.expect("install should succeed");
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl should be installed in the local db"
        );
    }

    #[test]
    #[ignore]
    fn test_install_multiple() {
        let mut handle = setup_fake_root("install_multiple");
        let result = install_into(
            &mut handle,
            &[
                InstallTarget::Repo("sl".to_string()),
                InstallTarget::Repo("figlet".to_string()),
            ],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        result.expect("install should succeed");
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl should be installed in the local db"
        );
        assert!(
            handle.localdb().pkg("figlet").is_ok(),
            "figlet should be installed in the local db"
        );
    }

    #[test]
    #[ignore]
    fn test_install_aborted() {
        let mut handle = setup_fake_root("abort");
        let result = install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || false,
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        result.expect("aborted install should not error");
        assert!(
            handle.localdb().pkg("sl").is_err(),
            "sl must NOT be installed after an aborted confirm"
        );
    }

    #[test]
    #[ignore]
    fn test_conflict_surfaces_as_named_abort() {
        let mut handle = setup_fake_root("conflict_abort");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("vim".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
        )
        .expect("vim should install first");
        assert!(
            handle.localdb().pkg("vim").is_ok(),
            "vim should be installed before the conflict test"
        );

        let result = install_into(
            &mut handle,
            &[InstallTarget::Repo("gvim".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        let err = result.expect_err("gvim install should abort due to vim conflict");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("declined to remove vim"),
            "error must name vim (the installed package) as removable: {msg}"
        );
        assert!(
            !msg.contains("failed to prepare transaction"),
            "friendly detail must win over the generic context: {msg}"
        );
        assert!(
            handle.localdb().pkg("gvim").is_err(),
            "gvim must NOT be installed after the aborted conflict"
        );
    }

    #[test]
    #[ignore]
    fn dry_run_captures_installed_conflict() {
        let mut handle = setup_fake_root("dryrun_conflict");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("cava".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
        )
        .expect("cava should install first");
        assert!(handle.localdb().pkg("cava").is_ok(), "cava installed");

        let cava_git = AurInfo {
            id: 1,
            name: "cava-git".into(),
            package_base_id: 2,
            package_base: "cava-git".into(),
            version: "0.10.4-1".into(),
            description: Some("console-based audio visualizer".into()),
            url: None,
            num_votes: 100,
            popularity: 5.0,
            out_of_date: None,
            maintainer: Some("someone".into()),
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: vec!["fftw".into()],
            make_depends: vec![],
            check_depends: vec![],
            opt_depends: vec![],
            conflicts: vec!["cava".into()],
            provides: vec![],
            replaces: vec![],
            groups: vec![],
            license: vec![],
            keywords: vec![],
            co_maintainers: vec![],
        };
        let plan = BuildPlan {
            targets: vec!["cava-git".to_string()],
            layers: vec![BuildLayer {
                aur: vec![cava_git],
                repo_deps: vec![],
            }],
        };

        let qs = crate::dry_run::dry_run(&mut handle, &plan).expect("dry_run should succeed");
        assert!(
            qs.conflicts
                .iter()
                .any(|c| c.incoming == "cava-git" && c.removable == "cava"),
            "should capture the cava-git vs cava conflict; got {qs:?}"
        );
    }

    #[test]
    #[ignore]
    fn test_provider_surfaces_choice() {
        use std::sync::{Arc, Mutex};

        use crate::answerer::{ConflictDecision, ProviderDecision, QuestionAnswerer};
        use crate::question::ProviderCandidate;

        struct RecordingAnswerer {
            recorded: Arc<Mutex<Vec<(String, Vec<ProviderCandidate>)>>>,
        }

        impl QuestionAnswerer for RecordingAnswerer {
            fn answer_conflict(
                &self,
                _incoming: &str,
                _incoming_version: &str,
                _removable: &str,
                _removable_version: &str,
            ) -> ConflictDecision {
                ConflictDecision::Decline
            }

            fn answer_provider(
                &self,
                depend: &str,
                candidates: &[ProviderCandidate],
            ) -> ProviderDecision {
                self.recorded
                    .lock()
                    .unwrap()
                    .push((depend.to_string(), candidates.to_vec()));
                ProviderDecision::Choose(0)
            }
        }

        let recorded: Arc<Mutex<Vec<(String, Vec<ProviderCandidate>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let mut handle = setup_fake_root("provider");
        let result = install_into(
            &mut handle,
            &[InstallTarget::Repo("sdl_net".to_string())],
            false,
            ConsoleSink::new(),
            || false,
            Box::new(RecordingAnswerer {
                recorded: recorded.clone(),
            }),
        );
        result.expect("install_into should succeed even when confirm aborts before commit");
        let captured = recorded.lock().unwrap().clone();
        println!("recorded provider prompts: {captured:?}");
        assert!(
            captured
                .iter()
                .any(|(depend, cands)| depend == "sdl" && cands.len() >= 2),
            "expected a SelectProvider for \"sdl\" with >=2 candidates; got {captured:?}"
        );
    }
}
