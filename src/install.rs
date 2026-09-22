use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Context;

use crate::events::{InstallEvent, InstallSink, LogLevel};
use crate::tx::convert::{
    convert_download, convert_event, convert_log_level, convert_progress_phase,
};

pub use crate::tx::convert::build_summary;

#[cfg(test)]
pub use tests::setup_fake_root;

pub struct QuestionState {
    pub deny_flag: bool,
    pub detail: String,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
}

impl QuestionState {
    pub fn new(answerer: Box<dyn crate::answerer::QuestionAnswerer>) -> Self {
        Self {
            deny_flag: false,
            detail: String::new(),
            answerer,
        }
    }
}

pub enum InstallTarget {
    Repo(String),
    File(PathBuf),
}

pub fn install_into<S: InstallSink + 'static, F: FnOnce() -> bool>(
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
    crate::pacman::lock::finish_transaction(handle);
    result
}

pub fn register_callbacks<S: InstallSink + 'static>(
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
                    spq.set_index(-1);
                    s.deny_flag = true;
                    s.detail = format!("declined to choose a provider for {depend}");
                }
                crate::answerer::ProviderDecision::CannotPrompt => {
                    spq.set_index(-1);
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
        alpm::Question::Replace(rq) => {
            rq.set_replace(true);
        }
        alpm::Question::Corrupted(mut cq) => {
            cq.set_remove(true);
        }
        alpm::Question::ImportKey(mut iq) => {
            iq.set_import(false);
        }
        alpm::Question::InstallIgnorepkg(mut iq) => {
            iq.set_install(false);
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
    crate::pacman::lock::cleanup_on_signal(handle);

    crate::pacman::lock::lock_retry(
        || handle.trans_init(alpm::TransFlag::NONE),
        || {
            sink.borrow_mut()
                .event(InstallEvent::WaitingForDatabaseLock);
        },
        crate::pacman::lock::LOCK_POLL_INTERVAL,
    )
    .context("failed to initialize transaction")?;

    if targets.iter().any(|t| matches!(t, InstallTarget::File(_))) {
        sink.borrow_mut().event(InstallEvent::LoadingPackages);
    }

    let repo_names: Vec<String> = targets
        .iter()
        .filter_map(|target| match target {
            InstallTarget::Repo(name) => Some(name.clone()),
            InstallTarget::File(_) => None,
        })
        .collect();
    let resolved = crate::tx::targets::resolve_targets(handle, &repo_names)?;
    for skipped in &resolved.skipped {
        sink.borrow_mut().event(InstallEvent::Log {
            level: LogLevel::Warning,
            message: format!("skipping target: {skipped}"),
        });
    }

    let mut added_names: Vec<String> = Vec::with_capacity(targets.len());
    for pkg in resolved.packages {
        added_names.push(pkg.name().to_string());
        handle
            .trans_add_pkg(pkg)
            .map_err(alpm::Error::from)
            .context("failed to queue package for installation")?;
    }
    for path in targets.iter().filter_map(|target| match target {
        InstallTarget::File(path) => Some(path),
        InstallTarget::Repo(_) => None,
    }) {
        let loaded = handle
            .pkg_load(
                path.to_string_lossy().as_ref(),
                true,
                crate::pacman::local_file_siglevel(handle),
            )
            .context("failed to load package file")?;
        added_names.push(loaded.name().to_string());
        handle
            .trans_add_pkg(loaded)
            .map_err(alpm::Error::from)
            .context("failed to queue package file for installation")?;
    }

    if handle.trans_add().is_empty() {
        sink.borrow_mut().event(InstallEvent::Log {
            level: LogLevel::Warning,
            message: "there is nothing to do".to_string(),
        });
        return Ok(());
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

    let commit = crate::pacman::lock::during_commit(|| handle.trans_commit());
    commit.context("failed to commit transaction")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConsoleSink;
    use crate::resolve::{Base, Member, Plan};
    use std::fs;

    pub fn setup_fake_root(suffix: &str) -> alpm::Alpm {
        let base = std::env::temp_dir().join(format!("pakajo_fake_root_{suffix}"));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        let cache = base.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&db).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let config = crate::pacman::config().unwrap();
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
    fn stub_targets_queue_without_conflict_metadata() {
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
        assert!(
            handle.localdb().pkg("cava").is_ok(),
            "cava should be installed before the stub conflict test"
        );
        let plan = Plan {
            bases: vec![Base::Aur {
                base: "cava-git".into(),
                build: true,
                members: vec![Member {
                    name: "cava-git".into(),
                    version: "0.10.4-1".into(),
                    make: false,
                    target: true,
                }],
            }],
            ..Default::default()
        };

        let state = crate::dry_run::attach_recorder(&mut handle);
        handle
            .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
            .expect("failed to init stub preview transaction");
        let (stub_dir, stubs) = crate::dispatch::install::build_stubs(Some(&plan))
            .expect("stub building should succeed");
        for stub in &stubs {
            let loaded = handle
                .pkg_load(stub.as_str(), false, alpm::SigLevel::NONE)
                .expect("stub should load");
            handle.trans_add_pkg(loaded).expect("stub should queue");
        }
        drop(stub_dir);
        handle
            .trans_prepare()
            .expect("stub transaction should prepare");
        let qs = crate::dry_run::snapshot(&state);
        let _ = handle.trans_release();
        assert!(
            qs.conflicts.is_empty(),
            "stubs carry no conflict metadata; got {qs:?}"
        );
    }

    #[test]
    #[ignore]
    fn stub_seed_without_metadata_installs_clean() {
        let mut handle = setup_fake_root("dryrun_repo_conflict");

        use crate::stub_pkg::build_stub_pkg;
        let stub_dir = tempfile::tempdir().unwrap();
        let stub = build_stub_pkg("cava-git", "0.10.4-1", stub_dir.path()).unwrap();
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        let loaded = handle
            .pkg_load(stub.to_string_lossy().as_ref(), false, alpm::SigLevel::NONE)
            .unwrap();
        handle.trans_add_pkg(loaded).unwrap();
        handle.trans_prepare().unwrap();
        handle.trans_commit().unwrap();
        handle.trans_release().unwrap();
        handle
            .localdb()
            .pkg("cava-git")
            .expect("cava-git should be installed in localdb after seeding");

        let request = crate::dispatch::InstallRequest {
            targets: vec!["cava".to_string()],
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: vec![],
            prefer_aur: false,
            decider: Box::new(crate::dispatch::protocol::AutomaticDecider::new()),
            approvals: None,
            tty: false,
            json: false,
        };
        let preview = crate::dispatch::install::run_install_preview(&mut handle, &request)
            .expect("install preview should succeed");
        let _ = handle.trans_release();
        let qs = preview.question_set();
        assert!(
            qs.conflicts.is_empty(),
            "stubs carry no conflict metadata; got {qs:?}"
        );
    }

    #[test]
    #[ignore]
    fn test_provider_surfaces_choice() {
        use std::sync::{Arc, Mutex};

        use crate::answerer::{ConflictDecision, ProviderDecision, QuestionAnswerer};
        use crate::question::ProviderCandidate;

        type RecordedProviders = Arc<Mutex<Vec<(String, Vec<ProviderCandidate>)>>>;

        struct RecordingAnswerer {
            recorded: RecordedProviders,
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

        let recorded: RecordedProviders = Arc::new(Mutex::new(Vec::new()));
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
