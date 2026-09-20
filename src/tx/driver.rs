use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context;

use crate::events::{InstallEvent, InstallSink, LogLevel, TransactionSummary};
use crate::pacman::lock::{
    LOCK_POLL_INTERVAL, cleanup_on_signal, during_commit, finish_transaction, lock_retry,
};
use crate::question::model::{Answer, Question};
use crate::question::source::AnswerSource;
use crate::tx::convert::{
    PrepareFailure, build_summary, convert_download, convert_event, convert_log_level,
    convert_progress_phase, extract_prepare_failure,
};
use crate::tx::questions::QuestionSession;
use crate::tx::targets::resolve_targets;

#[derive(Debug)]
pub enum RunKind {
    Sync,
}

#[derive(Debug)]
pub struct RunSpec {
    pub kind: RunKind,
    pub targets: Vec<String>,
    pub explore: bool,
    pub as_deps: bool,
    pub reinstall: bool,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub summary: TransactionSummary,
    pub finish: Finish,
}

#[derive(Debug)]
pub enum Finish {
    Committed,
    Stopped,
    PrepareFailed(PrepareFailure),
}

pub fn trans_init_flags(explore: bool, as_deps: bool, reinstall: bool) -> alpm::TransFlag {
    let mut flags = alpm::TransFlag::NONE;
    if !reinstall {
        flags |= alpm::TransFlag::NEEDED;
    }
    if as_deps {
        flags |= alpm::TransFlag::ALL_DEPS;
    }
    if explore {
        flags |= alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK;
    }
    flags
}

pub fn run(
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    source: Box<dyn AnswerSource>,
    sink: Box<dyn InstallSink>,
) -> anyhow::Result<RunOutcome> {
    let events: Rc<RefCell<Box<dyn InstallSink>>> = Rc::new(RefCell::new(sink));
    forward_alpm_events(handle, &events);
    cleanup_on_signal(handle);
    lock_retry(
        || handle.trans_init(trans_init_flags(spec.explore, spec.as_deps, spec.reinstall)),
        || {
            events
                .borrow_mut()
                .event(InstallEvent::WaitingForDatabaseLock);
        },
        LOCK_POLL_INTERVAL,
    )
    .context("failed to initialize transaction")?;
    let outcome = drive(handle, spec, source, &events);
    drop(handle.take_raw_question_cb());
    finish_transaction(handle);
    outcome
}

fn forward_alpm_events(handle: &mut alpm::Alpm, events: &Rc<RefCell<Box<dyn InstallSink>>>) {
    handle.set_event_cb(events.clone(), |any_event, data| {
        if let Some(event) = convert_event(any_event) {
            data.borrow_mut().event(event);
        }
    });
    handle.set_dl_cb(events.clone(), |filename, any_ev, data| {
        if let Some(event) = convert_download(filename, any_ev) {
            data.borrow_mut().event(event);
        }
    });
    handle.set_progress_cb(
        events.clone(),
        |phase, pkgname, percent, howmany, current, data| {
            data.borrow_mut().event(InstallEvent::Progress {
                phase: convert_progress_phase(phase),
                package: pkgname.to_string(),
                percent,
                current,
                total: howmany,
            });
        },
    );
    handle.set_log_cb(events.clone(), |level, message, data| {
        if let Some(mapped) = convert_log_level(level) {
            data.borrow_mut().event(InstallEvent::Log {
                level: mapped,
                message: message.to_string(),
            });
        }
    });
}

fn drive(
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    source: Box<dyn AnswerSource>,
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
) -> anyhow::Result<RunOutcome> {
    let session = QuestionSession::attach(handle, source);
    if spec.targets.iter().any(|target| is_file_target(target)) {
        events.borrow_mut().event(InstallEvent::LoadingPackages);
    }
    queue_targets(handle, spec, &session)?;
    if handle.trans_add().is_empty() {
        fail_on_denied(&session.borrow())?;
        if !spec.explore {
            events.borrow_mut().event(InstallEvent::Log {
                level: LogLevel::Warning,
                message: " there is nothing to do".to_string(),
            });
        }
        return Ok(outcome(handle, Finish::Stopped));
    }
    let failure = match handle.trans_prepare() {
        Ok(()) => None,
        Err(error) => Some(extract_prepare_failure(error)),
    };
    if let Some(failure) = failure {
        fail_on_denied(&session.borrow())?;
        return Ok(outcome(handle, Finish::PrepareFailed(failure)));
    }
    fail_on_denied(&session.borrow())?;
    let summary = build_summary(handle);
    if !spec.explore {
        events
            .borrow_mut()
            .event(InstallEvent::TransactionSummary(summary.clone()));
    }
    if spec.explore || !ask_proceed(&session, &summary)? {
        return Ok(RunOutcome {
            summary,
            finish: Finish::Stopped,
        });
    }
    during_commit(|| handle.trans_commit()).context("failed to commit transaction")?;
    fail_on_denied(&session.borrow())?;
    Ok(RunOutcome {
        summary,
        finish: Finish::Committed,
    })
}

fn ask_proceed(
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
) -> anyhow::Result<bool> {
    let question = Question::Proceed(summary.clone());
    let key = question.key();
    let asked = session.borrow_mut().ask_direct(&question)?;
    let Some(answer) = asked else {
        return Ok(false);
    };
    let Answer::Proceed = answer else {
        session
            .borrow_mut()
            .deny(key, "answer did not match question");
        return Err(fail_on_denied(&session.borrow()).unwrap_err());
    };
    Ok(true)
}

fn queue_targets(
    handle: &alpm::Alpm,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<()> {
    for target in &spec.targets {
        if is_file_target(target) {
            queue_file(handle, target)?;
        } else if let Some(members) = group_members(handle, target) {
            queue_group(handle, target, &members, session)?;
        } else {
            queue_named(handle, target)?;
        }
        fail_on_denied(&session.borrow())?;
    }
    Ok(())
}

fn is_file_target(target: &str) -> bool {
    target.contains(std::path::MAIN_SEPARATOR) && std::path::Path::new(target).exists()
}

fn queue_file(handle: &alpm::Alpm, target: &str) -> anyhow::Result<()> {
    let loaded = handle
        .pkg_load(target, true, crate::pacman::local_file_siglevel(handle))
        .context("failed to load package file")?;
    handle
        .trans_add_pkg(loaded)
        .map_err(alpm::Error::from)
        .context("failed to queue package file for installation")?;
    Ok(())
}

fn group_members(handle: &alpm::Alpm, target: &str) -> Option<Vec<String>> {
    let mut found = false;
    let mut members = Vec::new();
    for db in handle.syncdbs().iter() {
        let Ok(group) = db.group(target) else {
            continue;
        };
        found = true;
        for pkg in group.packages().iter() {
            let name = pkg.name().to_string();
            if !members.contains(&name) {
                members.push(name);
            }
        }
    }
    found.then_some(members)
}

fn queue_group(
    handle: &alpm::Alpm,
    target: &str,
    members: &[String],
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<()> {
    let question = Question::GroupMembers {
        group: target.to_string(),
        members: members.to_vec(),
    };
    let key = question.key();
    let asked = session.borrow_mut().ask_direct(&question)?;
    let Some(answer) = asked else {
        return Ok(());
    };
    let Answer::GroupMembers { selected } = answer else {
        session
            .borrow_mut()
            .deny(key, "answer did not match question");
        return fail_on_denied(&session.borrow());
    };
    if let Some(foreign) = selected.iter().find(|name| !members.contains(name)) {
        session
            .borrow_mut()
            .deny(key, format!("group member {foreign} is not offered"));
        return fail_on_denied(&session.borrow());
    }
    for name in &selected {
        queue_named(handle, name)?;
    }
    Ok(())
}

fn queue_named(handle: &alpm::Alpm, target: &str) -> anyhow::Result<()> {
    let resolved = resolve_targets(handle, std::slice::from_ref(&target.to_string()))?;
    for pkg in resolved.packages {
        handle
            .trans_add_pkg(pkg)
            .map_err(alpm::Error::from)
            .context("failed to queue package for installation")?;
    }
    Ok(())
}

fn outcome(handle: &alpm::Alpm, finish: Finish) -> RunOutcome {
    RunOutcome {
        summary: build_summary(handle),
        finish,
    }
}

fn fail_on_denied(session: &QuestionSession) -> anyhow::Result<()> {
    match session.denied() {
        Some(denied) => anyhow::bail!("{denied}"),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{InstallEvent, InstallSink, LogLevel};
    use crate::question::model::{Answer, Question};
    use crate::question::source::{FailClosed, SourceDecision};
    use std::cell::Cell;
    use std::fs::File;
    use std::path::Path;

    struct Script {
        respond: Box<dyn Fn(&Question) -> SourceDecision>,
    }

    impl AnswerSource for Script {
        fn answer(&self, question: &Question) -> SourceDecision {
            (self.respond)(question)
        }
    }

    fn script(respond: impl Fn(&Question) -> SourceDecision + 'static) -> Box<dyn AnswerSource> {
        Box::new(Script {
            respond: Box::new(respond),
        })
    }

    fn deny() -> Box<dyn AnswerSource> {
        script(|question| {
            SourceDecision::Abort(FailClosed {
                key: question.key(),
                reason: "denied in test".to_string(),
            })
        })
    }

    fn stop() -> Box<dyn AnswerSource> {
        script(|_| SourceDecision::Answer(Answer::Stop))
    }

    fn proceed() -> Box<dyn AnswerSource> {
        script(|question| match question {
            Question::Proceed(_) => SourceDecision::Answer(Answer::Proceed),
            _ => SourceDecision::Answer(Answer::Stop),
        })
    }

    fn group_answer(selected: &[&str]) -> Box<dyn AnswerSource> {
        let owned: Vec<String> = selected.iter().map(|name| name.to_string()).collect();
        script(move |_| {
            SourceDecision::Answer(Answer::GroupMembers {
                selected: owned.clone(),
            })
        })
    }

    struct Discard;

    impl InstallSink for Discard {
        fn event(&mut self, _event: InstallEvent) {}
    }

    fn discard() -> Box<dyn InstallSink> {
        Box::new(Discard)
    }

    #[derive(Clone, Default)]
    struct Recorder {
        seen: Rc<RefCell<Vec<InstallEvent>>>,
    }

    impl InstallSink for Recorder {
        fn event(&mut self, event: InstallEvent) {
            self.seen.borrow_mut().push(event);
        }
    }

    struct OrderSink {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl InstallSink for OrderSink {
        fn event(&mut self, event: InstallEvent) {
            if matches!(event, InstallEvent::TransactionSummary(_)) {
                self.log.borrow_mut().push("summary".to_string());
            }
        }
    }

    fn spec(targets: &[&str], explore: bool) -> RunSpec {
        RunSpec {
            kind: RunKind::Sync,
            targets: targets.iter().map(|t| t.to_string()).collect(),
            explore,
            as_deps: false,
            reinstall: false,
        }
    }

    #[derive(Clone)]
    struct Pkg {
        name: &'static str,
        version: &'static str,
        depends: Vec<&'static str>,
        provides: Vec<&'static str>,
        conflicts: Vec<&'static str>,
        groups: Vec<&'static str>,
    }

    fn make(
        name: &'static str,
        version: &'static str,
        depends: &[&'static str],
        provides: &[&'static str],
        groups: &[&'static str],
    ) -> Pkg {
        Pkg {
            name,
            version,
            depends: depends.to_vec(),
            provides: provides.to_vec(),
            conflicts: Vec::new(),
            groups: groups.to_vec(),
        }
    }

    fn desc(package: &Pkg) -> Vec<u8> {
        let mut out = format!(
            "%NAME%\n{}\n\n%VERSION%\n{}\n\n%FILENAME%\n{}\n\n",
            package.name,
            package.version,
            crate::tx::targets::filename(package.name, package.version),
        );
        for (tag, entries) in [
            ("%DEPENDS%\n", &package.depends),
            ("%CONFLICTS%\n", &package.conflicts),
            ("%PROVIDES%\n", &package.provides),
            ("%GROUPS%\n", &package.groups),
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

    fn write_syncdb(dbpath: &Path, repo: &str, packages: &[Pkg]) {
        let file = File::create(dbpath.join("sync").join(format!("{repo}.db"))).unwrap();
        let mut builder = tar::Builder::new(file);
        for package in packages {
            let content = desc(package);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{}-{}/desc", package.name, package.version),
                    content.as_slice(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
    }

    fn fixture_full(sync: &[(&str, Vec<Pkg>)], local: &[Pkg]) -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        for (repo, packages) in sync {
            write_syncdb(&db, repo, packages);
            for package in packages {
                crate::tx::targets::write_cachedir_stub(
                    &cache,
                    package.name,
                    package.version,
                    &package.depends,
                    &package.conflicts,
                );
            }
        }
        for package in local {
            let dir_path = db
                .join("local")
                .join(format!("{}-{}", package.name, package.version));
            std::fs::create_dir_all(&dir_path).unwrap();
            std::fs::write(dir_path.join("desc"), desc(package)).unwrap();
            std::fs::write(dir_path.join("files"), b"%FILES%\n").unwrap();
        }
        for (repo, _) in sync {
            handle
                .register_syncdb_mut(*repo, alpm::SigLevel::NONE)
                .unwrap()
                .add_server("file:///pakajo-offline-stub")
                .unwrap();
        }
        handle
            .add_cachedir(cache.to_string_lossy().as_ref())
            .unwrap();
        (dir, handle)
    }

    fn fixture(packages: &[Pkg]) -> (tempfile::TempDir, alpm::Alpm) {
        fixture_full(&[("core", packages.to_vec())], &[])
    }

    fn plain(name: &'static str) -> Pkg {
        make(name, "1.0-1", &[], &[], &[])
    }

    fn tools_fixture() -> (tempfile::TempDir, alpm::Alpm) {
        fixture(&[
            make("a", "1.0-1", &[], &[], &["tools"]),
            make("b", "1.0-1", &[], &[], &["tools"]),
            make("c", "1.0-1", &[], &[], &["tools"]),
        ])
    }

    fn summary_names(outcome: &RunOutcome) -> Vec<String> {
        let mut names: Vec<String> = outcome
            .summary
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect();
        names.sort();
        names
    }

    fn release(handle: &mut alpm::Alpm) {
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }

    fn peer(dbpath: &Path) -> alpm::Alpm {
        alpm::Alpm::new("/", dbpath.to_string_lossy().as_ref()).unwrap()
    }

    #[test]
    fn init_flags_derive_from_spec() {
        use alpm::TransFlag as F;
        for (explore, as_deps, reinstall, expected) in [
            (false, false, false, F::NEEDED),
            (false, false, true, F::NONE),
            (false, true, false, F::NEEDED | F::ALL_DEPS),
            (false, true, true, F::ALL_DEPS),
            (true, false, false, F::NEEDED | F::DB_ONLY | F::NO_LOCK),
            (true, false, true, F::DB_ONLY | F::NO_LOCK),
            (
                true,
                true,
                false,
                F::NEEDED | F::ALL_DEPS | F::DB_ONLY | F::NO_LOCK,
            ),
            (true, true, true, F::ALL_DEPS | F::DB_ONLY | F::NO_LOCK),
        ] {
            assert_eq!(trans_init_flags(explore, as_deps, reinstall), expected);
        }
    }

    #[test]
    fn explore_runs_under_foreign_lock_without_committing() {
        let (_dir, mut handle) = fixture(&[plain("foo")]);
        let mut holder = peer(Path::new(handle.dbpath()));
        holder.trans_init(alpm::TransFlag::NONE).unwrap();
        let outcome = run(&mut handle, &spec(&["foo"], true), deny(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(summary_names(&outcome), vec!["foo".to_string()]);
        assert!(handle.localdb().pkg("foo").is_err());
        holder.trans_release().unwrap();
        release(&mut handle);
    }

    #[test]
    fn denied_question_aborts_with_reason() {
        let (_dir, mut handle) = fixture(&[plain("skipme")]);
        handle.add_ignorepkg("skipme").unwrap();
        let error = run(&mut handle, &spec(&["skipme"], false), deny(), discard()).unwrap_err();
        assert!(format!("{error:#}").contains("denied in test"));
        release(&mut handle);
    }

    #[test]
    fn unsatisfiable_dep_reports_prepare_failure() {
        let (_dir, mut handle) = fixture(&[make("needy", "1.0-1", &["ghost>=9"], &[], &[])]);
        let outcome = run(&mut handle, &spec(&["needy"], false), stop(), discard()).unwrap();
        let Finish::PrepareFailed(PrepareFailure::Unsatisfied(missing)) = outcome.finish else {
            panic!("expected an unsatisfied prepare failure");
        };
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].depend, "ghost");
        release(&mut handle);
    }

    #[test]
    fn file_target_loads_and_queues() {
        let (_dir, mut handle) = fixture(&[]);
        let stub_dir = tempfile::tempdir().unwrap();
        let stub =
            crate::stub_pkg::build_stub_pkg("cava-git", "0.10.4-1", stub_dir.path()).unwrap();
        let target = stub.to_string_lossy().into_owned();
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let outcome = run(
            &mut handle,
            &spec(&[&target], true),
            stop(),
            Box::new(recorder),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(summary_names(&outcome), vec!["cava-git".to_string()]);
        assert!(
            seen.borrow()
                .iter()
                .any(|event| matches!(event, InstallEvent::LoadingPackages))
        );
        release(&mut handle);
    }

    #[test]
    fn group_selection_expands_subset_empty_stops_and_spans_dbs() {
        let (_dir, mut handle) = tools_fixture();
        let outcome = run(
            &mut handle,
            &spec(&["tools"], true),
            group_answer(&["a", "c"]),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(
            summary_names(&outcome),
            vec!["a".to_string(), "c".to_string()]
        );
        release(&mut handle);

        let (_dir, mut handle) = tools_fixture();
        let outcome = run(
            &mut handle,
            &spec(&["tools"], true),
            group_answer(&[]),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(outcome.summary.packages.is_empty());
        release(&mut handle);

        let sync = [
            ("core", vec![make("a", "1.0-1", &[], &[], &["tools"])]),
            ("extra", vec![make("b", "1.0-1", &[], &[], &["tools"])]),
        ];
        let (_dir, mut handle) = fixture_full(&sync, &[]);
        let outcome = run(
            &mut handle,
            &spec(&["tools"], true),
            group_answer(&["a", "b"]),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(
            summary_names(&outcome),
            vec!["a".to_string(), "b".to_string()]
        );
        release(&mut handle);
    }

    #[test]
    fn mismatched_answers_fail_closed() {
        let (_dir, mut handle) = tools_fixture();
        let error = run(
            &mut handle,
            &spec(&["tools"], false),
            group_answer(&["ghost"]),
            discard(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("is not offered"));
        release(&mut handle);

        let (_dir, mut handle) = tools_fixture();
        let mismatched = script(|_| {
            SourceDecision::Answer(Answer::Conflict {
                incoming: "a".to_string(),
                removable: "b".to_string(),
                remove: true,
            })
        });
        let error = run(&mut handle, &spec(&["tools"], false), mismatched, discard()).unwrap_err();
        assert!(format!("{error:#}").contains("did not match"));
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[plain("solo")]);
        let mismatched = script(|_| {
            SourceDecision::Answer(Answer::GroupMembers {
                selected: Vec::new(),
            })
        });
        let error = run(&mut handle, &spec(&["solo"], false), mismatched, discard()).unwrap_err();
        assert!(format!("{error:#}").contains("Proceed"));
        assert!(handle.localdb().pkg("solo").is_err());
        release(&mut handle);
    }

    #[test]
    fn provider_answer_resolves_by_name_across_orders() {
        for order in [
            vec![
                make("provider-one", "1.0-1", &[], &["virt"], &[]),
                make("provider-two", "1.0-1", &[], &["virt"], &[]),
            ],
            vec![
                make("provider-two", "1.0-1", &[], &["virt"], &[]),
                make("provider-one", "1.0-1", &[], &["virt"], &[]),
            ],
        ] {
            let (_dir, mut handle) = fixture(&order);
            let source = script(|question| match question {
                Question::SelectProvider { .. } => SourceDecision::Answer(Answer::SelectProvider {
                    name: "provider-two".to_string(),
                    repo: Some("core".to_string()),
                }),
                _ => SourceDecision::Answer(Answer::Stop),
            });
            let outcome = run(&mut handle, &spec(&["virt"], true), source, discard()).unwrap();
            assert!(matches!(outcome.finish, Finish::Stopped));
            assert_eq!(summary_names(&outcome), vec!["provider-two".to_string()]);
            release(&mut handle);
        }
    }

    #[test]
    fn needed_skips_installed_and_reinstall_requeues() {
        let sync = [plain("foo")];
        let local = [plain("foo")];
        let (_dir, mut handle) = fixture_full(&[("core", sync.to_vec())], &local);
        let outcome = run(&mut handle, &spec(&["foo"], true), stop(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(outcome.summary.packages.is_empty());
        release(&mut handle);

        let (_dir, mut handle) = fixture_full(&[("core", sync.to_vec())], &local);
        let reinstall = RunSpec {
            targets: vec!["foo".to_string()],
            explore: true,
            as_deps: false,
            reinstall: true,
            ..spec(&[], true)
        };
        let outcome = run(&mut handle, &reinstall, stop(), discard()).unwrap();
        assert_eq!(summary_names(&outcome), vec!["foo".to_string()]);
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[plain("skipme")]);
        handle.add_ignorepkg("skipme").unwrap();
        let outcome = run(&mut handle, &spec(&["skipme"], false), stop(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(outcome.summary.packages.is_empty());
        release(&mut handle);
    }

    #[test]
    fn commit_marks_explicit_depend_and_as_deps_reasons() {
        let sync = vec![make("app", "1.0-1", &["lib"], &[], &[]), plain("lib")];
        let (_dir, mut handle) = fixture_full(&[("core", sync)], &[]);
        let outcome = run(&mut handle, &spec(&["app"], false), proceed(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert_eq!(
            handle.localdb().pkg("app").unwrap().reason(),
            alpm::PackageReason::Explicit
        );
        assert_eq!(
            handle.localdb().pkg("lib").unwrap().reason(),
            alpm::PackageReason::Depend
        );
        release(&mut handle);

        let (_dir, mut handle) = fixture_full(&[("core", vec![plain("solo")])], &[]);
        let as_deps = RunSpec {
            targets: vec!["solo".to_string()],
            explore: false,
            as_deps: true,
            reinstall: false,
            ..spec(&[], false)
        };
        let outcome = run(&mut handle, &as_deps, proceed(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert_eq!(
            handle.localdb().pkg("solo").unwrap().reason(),
            alpm::PackageReason::Depend
        );
        release(&mut handle);
    }

    #[test]
    fn commit_conflict_removal_and_declined_proceed_stop() {
        let conflicting = Pkg {
            version: "2.0-1",
            conflicts: vec!["oldpkg"],
            ..plain("newpkg")
        };
        let (_dir, mut handle) = fixture_full(&[("core", vec![conflicting])], &[plain("oldpkg")]);
        let source = script(|question| match question {
            Question::Conflict {
                incoming,
                removable,
            } => SourceDecision::Answer(Answer::Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
                remove: true,
            }),
            Question::Proceed(_) => SourceDecision::Answer(Answer::Proceed),
            _ => SourceDecision::Answer(Answer::Stop),
        });
        let outcome = run(&mut handle, &spec(&["newpkg"], false), source, discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(handle.localdb().pkg("newpkg").is_ok());
        assert!(handle.localdb().pkg("oldpkg").is_err());
        assert!(
            outcome
                .summary
                .packages
                .iter()
                .any(|package| package.name == "oldpkg" && package.is_removal)
        );
        release(&mut handle);

        let (_dir, mut handle) = fixture_full(&[("core", vec![plain("solo")])], &[]);
        let outcome = run(&mut handle, &spec(&["solo"], false), stop(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(handle.localdb().pkg("solo").is_err());
        release(&mut handle);
    }

    #[test]
    fn first_denial_wins_across_questions() {
        let (_dir, mut handle) = fixture(&[plain("aaa"), plain("bbb")]);
        handle.add_ignorepkg("aaa").unwrap();
        handle.add_ignorepkg("bbb").unwrap();
        let calls = Rc::new(Cell::new(0));
        let first = Rc::new(RefCell::new(String::new()));
        let source = {
            let calls = Rc::clone(&calls);
            let first = Rc::clone(&first);
            Box::new(Script {
                respond: Box::new(move |question: &Question| {
                    calls.set(calls.get() + 1);
                    let Question::InstallIgnorepkg { name } = question else {
                        return SourceDecision::Answer(Answer::Stop);
                    };
                    if calls.get() == 1 {
                        *first.borrow_mut() = name.clone();
                        return SourceDecision::Abort(FailClosed {
                            key: question.key(),
                            reason: "first denial".to_string(),
                        });
                    }
                    SourceDecision::Answer(Answer::InstallIgnorepkg {
                        name: name.clone(),
                        install: true,
                    })
                }),
            }) as Box<dyn AnswerSource>
        };
        let error = run(
            &mut handle,
            &spec(&["aaa", "bbb"], false),
            source,
            discard(),
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert!(format!("{error:#}").contains(&*first.borrow()));
        release(&mut handle);
    }

    #[test]
    fn tty_event_lifecycle() {
        let (_dir, mut handle) = fixture(&[plain("solo")]);
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let asked = log.clone();
        let source = script(move |question| {
            if matches!(question, Question::Proceed(_)) {
                asked.borrow_mut().push("question".to_string());
                SourceDecision::Answer(Answer::Proceed)
            } else {
                SourceDecision::Answer(Answer::Stop)
            }
        });
        let outcome = run(
            &mut handle,
            &spec(&["solo"], false),
            source,
            Box::new(OrderSink { log: log.clone() }),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert_eq!(
            *log.borrow(),
            vec!["summary".to_string(), "question".to_string()]
        );
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[plain("foo")]);
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let outcome = run(&mut handle, &spec(&[], false), stop(), Box::new(recorder)).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        match &seen[0] {
            InstallEvent::Log { level, message } => {
                assert_eq!(*level, LogLevel::Warning);
                assert_eq!(message, " there is nothing to do");
            }
            other => panic!("expected nothing-to-do log, got {other:?}"),
        }
        release(&mut handle);
    }
}
