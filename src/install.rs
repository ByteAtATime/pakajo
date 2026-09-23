use std::cell::RefCell;
use std::rc::Rc;

use crate::events::{InstallEvent, InstallSink};
use crate::tx::convert::{
    convert_download, convert_event, convert_log_level, convert_progress_phase,
};

#[cfg(test)]
pub use tests::{OfflinePkg, drive_sync, offline_pkg, offline_root, setup_fake_root};

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
    File(std::path::PathBuf),
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

#[cfg(test)]
mod tests {
    use crate::question::model::{Answer, Question};
    use crate::question::source::{AnswerSource, ExploreDefaults, SourceDecision};
    use crate::tx::driver::{Finish, RunKind, RunOutcome, RunSpec};
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

    pub struct OfflinePkg {
        pub name: &'static str,
        pub depends: &'static [&'static str],
        pub provides: &'static [&'static str],
        pub conflicts: &'static [&'static str],
        pub groups: &'static [&'static str],
    }

    pub fn offline_pkg(name: &'static str) -> OfflinePkg {
        OfflinePkg {
            name,
            depends: &[],
            provides: &[],
            conflicts: &[],
            groups: &[],
        }
    }

    pub fn offline_root(packages: &[OfflinePkg]) -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(db.join("local")).unwrap();
        fs::create_dir_all(db.join("sync")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        let file = fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        for package in packages {
            let content = offline_desc(package);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{}-1.0-1/desc", package.name),
                    content.as_slice(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
        for package in packages {
            crate::tx::targets::write_cachedir_stub(
                &cache,
                package.name,
                "1.0-1",
                package.depends,
                package.provides,
                package.conflicts,
                package.groups,
            );
        }
        handle
            .register_syncdb_mut("core", alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
        handle
            .add_cachedir(cache.to_string_lossy().as_ref())
            .unwrap();
        (dir, handle)
    }

    fn offline_desc(package: &OfflinePkg) -> Vec<u8> {
        let mut out = format!(
            "%NAME%\n{}\n\n%VERSION%\n1.0-1\n\n%FILENAME%\n{}\n\n",
            package.name,
            crate::tx::targets::filename(package.name, "1.0-1"),
        );
        for (tag, entries) in [
            ("%DEPENDS%\n", package.depends),
            ("%CONFLICTS%\n", package.conflicts),
            ("%PROVIDES%\n", package.provides),
            ("%GROUPS%\n", package.groups),
        ] {
            if entries.is_empty() {
                continue;
            }
            out.push_str(tag);
            for entry in entries {
                out.push_str(entry);
                out.push('\n');
            }
            out.push('\n');
        }
        out.into_bytes()
    }

    struct Discard;

    impl crate::events::InstallSink for Discard {
        fn event(&mut self, _event: crate::events::InstallEvent) {}
    }

    pub fn drive_sync(
        handle: &mut alpm::Alpm,
        targets: &[&str],
        source: Box<dyn AnswerSource>,
    ) -> anyhow::Result<RunOutcome> {
        let spec = RunSpec {
            kind: RunKind::Sync,
            targets: targets.iter().map(|target| target.to_string()).collect(),
            stub_targets: Vec::new(),
            explore: false,
            as_deps: false,
            reinstall: false,
            dep_names: Vec::new(),
        };
        crate::tx::driver::run(handle, &spec, source, Box::new(Discard))
    }

    fn preapproved() -> Box<dyn AnswerSource> {
        crate::tx::prompt::with_preapproved_proceed(Box::new(ExploreDefaults))
    }

    struct DeclineConflicts;

    impl AnswerSource for DeclineConflicts {
        fn answer(&self, question: &Question) -> SourceDecision {
            match question {
                Question::Conflict {
                    incoming,
                    removable,
                } => SourceDecision::Answer(Answer::Conflict {
                    incoming: incoming.clone(),
                    removable: removable.clone(),
                    remove: false,
                }),
                other => ExploreDefaults.answer(other),
            }
        }
    }

    #[test]
    fn engine_install_commits_single_package() {
        let (_dir, mut handle) = offline_root(&[offline_pkg("sl")]);
        let outcome = drive_sync(&mut handle, &["sl"], preapproved()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl should be installed in the local db"
        );
    }

    #[test]
    fn engine_install_commits_multiple_targets() {
        let (_dir, mut handle) = offline_root(&[offline_pkg("sl"), offline_pkg("figlet")]);
        let outcome = drive_sync(&mut handle, &["sl", "figlet"], preapproved()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
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
    fn engine_install_stopped_leaves_localdb_empty() {
        let (_dir, mut handle) = offline_root(&[offline_pkg("sl")]);
        let outcome = drive_sync(&mut handle, &["sl"], Box::new(ExploreDefaults)).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(
            handle.localdb().pkg("sl").is_err(),
            "sl must NOT be installed after a stopped confirm"
        );
    }

    #[test]
    fn engine_conflict_decline_fails_without_committing() {
        let gvim = OfflinePkg {
            name: "gvim",
            conflicts: &["vim"],
            ..offline_pkg("gvim")
        };
        let (_dir, mut handle) = offline_root(&[offline_pkg("vim"), gvim]);
        drive_sync(&mut handle, &["vim"], preapproved()).unwrap();
        assert!(
            handle.localdb().pkg("vim").is_ok(),
            "vim should be installed before the conflict test"
        );

        let outcome = drive_sync(&mut handle, &["gvim"], Box::new(DeclineConflicts)).unwrap();
        assert!(
            matches!(outcome.finish, Finish::PrepareFailed(_)),
            "declined conflict must fail prepare, got {:?}",
            outcome.finish
        );
        assert!(
            handle.localdb().pkg("gvim").is_err(),
            "gvim must NOT be installed after the declined conflict"
        );
    }

    #[test]
    fn engine_provider_choice_is_recorded() {
        use std::sync::{Arc, Mutex};

        type RecordedProviders = Arc<Mutex<Vec<(String, usize)>>>;

        struct RecordingProvider {
            recorded: RecordedProviders,
        }

        impl AnswerSource for RecordingProvider {
            fn answer(&self, question: &Question) -> SourceDecision {
                match question {
                    Question::SelectProvider { depend, candidates } => {
                        self.recorded
                            .lock()
                            .unwrap()
                            .push((depend.clone(), candidates.len()));
                        let first = &candidates[0];
                        SourceDecision::Answer(Answer::SelectProvider {
                            name: first.name.clone(),
                            repo: first.repo.clone(),
                        })
                    }
                    other => ExploreDefaults.answer(other),
                }
            }
        }

        let netapp = OfflinePkg {
            name: "netapp",
            depends: &["sdl"],
            ..offline_pkg("netapp")
        };
        let sdl_one = OfflinePkg {
            name: "sdl-one",
            provides: &["sdl"],
            ..offline_pkg("sdl-one")
        };
        let sdl_two = OfflinePkg {
            name: "sdl-two",
            provides: &["sdl"],
            ..offline_pkg("sdl-two")
        };
        let (_dir, mut handle) = offline_root(&[netapp, sdl_one, sdl_two]);
        let recorded: RecordedProviders = Arc::new(Mutex::new(Vec::new()));
        let outcome = drive_sync(
            &mut handle,
            &["netapp"],
            Box::new(RecordingProvider {
                recorded: recorded.clone(),
            }),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let captured = recorded.lock().unwrap().clone();
        assert!(
            captured
                .iter()
                .any(|(depend, count)| depend == "sdl" && *count >= 2),
            "expected a SelectProvider for \"sdl\" with >=2 candidates; got {captured:?}"
        );
    }
}
