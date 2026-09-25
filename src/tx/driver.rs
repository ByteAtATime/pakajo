use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context;

use crate::events::{InstallEvent, InstallSink, LogLevel, TransactionSummary};
use crate::pacman::lock::{
    LOCK_POLL_INTERVAL, cleanup_on_signal, during_commit, finish_transaction, lock_retry,
};
use crate::question::model::{Answer, Question, QuestionKey, TransactionKind};
use crate::question::source::AnswerSource;
use crate::tx::convert::{
    PrepareFailure, build_summary, convert_download, convert_event, convert_log_level,
    convert_progress_phase, extract_prepare_failure,
};
use crate::tx::questions::QuestionSession;
use crate::tx::targets::{is_file_target, resolve_targets};

#[derive(Debug, Clone)]
pub struct RemoveSpec {
    pub flags: alpm::TransFlag,
    pub holds: Vec<String>,
}

#[derive(Debug)]
pub enum RunKind {
    Sync,
    Upgrade,
    Remove(RemoveSpec),
}

#[derive(Debug)]
pub struct RunSpec {
    pub kind: RunKind,
    pub targets: Vec<String>,
    pub stub_targets: Vec<String>,
    pub explore: bool,
    pub as_deps: bool,
    pub reinstall: bool,
    pub dep_names: Vec<String>,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub summary: TransactionSummary,
    pub finish: Finish,
    pub review: Option<crate::question::review::Review>,
}

#[derive(Debug)]
pub enum Finish {
    Committed,
    Stopped,
    PrepareFailed(PrepareFailure),
}

pub fn trans_init_flags(spec: &RunSpec) -> alpm::TransFlag {
    let mut flags = match &spec.kind {
        RunKind::Sync if !spec.reinstall => alpm::TransFlag::NEEDED,
        RunKind::Sync | RunKind::Upgrade => alpm::TransFlag::NONE,
        RunKind::Remove(remove) => remove.flags,
    };
    if spec.as_deps {
        flags |= alpm::TransFlag::ALL_DEPS;
    }
    if spec.explore {
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
    run_with_events(handle, spec, source, &events)
}

pub fn run_with_events(
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    source: Box<dyn AnswerSource>,
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
) -> anyhow::Result<RunOutcome> {
    forward_alpm_events(handle, events);
    cleanup_on_signal(handle);
    lock_retry(
        || handle.trans_init(trans_init_flags(spec)),
        || {
            events
                .borrow_mut()
                .event(InstallEvent::WaitingForDatabaseLock);
        },
        LOCK_POLL_INTERVAL,
    )
    .context("failed to initialize transaction")?;
    let outcome = drive(handle, spec, source, events);
    drop(handle.take_raw_question_cb());
    finish_transaction(handle);
    outcome
}

pub(crate) fn forward_alpm_events(
    handle: &mut alpm::Alpm,
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
) {
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
    if matches!(spec.kind, RunKind::Sync | RunKind::Upgrade)
        && spec
            .stub_targets
            .iter()
            .chain(spec.targets.iter())
            .any(|target| is_file_target(target))
    {
        events.borrow_mut().event(InstallEvent::LoadingPackages);
    }
    let queue_result: anyhow::Result<Vec<String>> = match &spec.kind {
        RunKind::Sync => queue_targets(handle, spec, &session).map(|()| Vec::new()),
        RunKind::Upgrade => {
            if !spec.explore {
                events.borrow_mut().event(InstallEvent::StartSysupgrade);
            }
            queue_upgrade(handle, spec, &session).map(|()| Vec::new())
        }
        RunKind::Remove(_) => queue_remove_targets(handle, &spec.targets),
    };
    let missing = closed(&session, events, queue_result)?;
    closed(&session, events, ask_missing_removal(&session, &missing))?;
    let nothing_queued = match &spec.kind {
        RunKind::Sync => handle.trans_add().is_empty(),
        RunKind::Upgrade => handle.trans_add().is_empty() && handle.trans_remove().is_empty(),
        RunKind::Remove(_) => handle.trans_remove().is_empty(),
    };
    if nothing_queued {
        fail_closed(&session, events)?;
        if !spec.explore && !matches!(spec.kind, RunKind::Upgrade) {
            events.borrow_mut().event(InstallEvent::Log {
                level: LogLevel::Warning,
                message: "there is nothing to do".to_string(),
            });
        }
        return outcome(handle, Finish::Stopped, spec, &session);
    }
    let failure = handle.trans_prepare().err().map(extract_prepare_failure);
    if let Some(failure) = failure {
        let _ = fail_closed(&session, events);
        return outcome(handle, Finish::PrepareFailed(failure), spec, &session);
    }
    fail_closed(&session, events)?;
    if let RunKind::Remove(remove) = &spec.kind {
        let names: Vec<String> = handle
            .trans_remove()
            .iter()
            .map(|pkg| pkg.name().to_string())
            .collect();
        let held = crate::holdpkg::held_packages(&names, &remove.holds);
        closed(&session, events, ask_held_removal(&session, &held))?;
    }
    let summary = build_summary(handle);
    if !spec.explore {
        events
            .borrow_mut()
            .event(InstallEvent::TransactionSummary(summary.clone()));
    }
    let review = review(handle, spec, &session, &summary)?;
    let commit = !spec.explore
        && closed(
            &session,
            events,
            ask_proceed(&session, &summary, &spec.kind),
        )?;
    if !commit {
        return Ok(RunOutcome {
            summary,
            finish: Finish::Stopped,
            review,
        });
    }
    let fresh: Vec<&String> = spec
        .dep_names
        .iter()
        .filter(|name| handle.localdb().pkg(name.as_str()).is_err())
        .collect();
    during_commit(|| handle.trans_commit()).context("failed to commit transaction")?;
    apply_dep_reasons(handle, &fresh, events);
    fail_closed(&session, events)?;
    Ok(RunOutcome {
        summary,
        finish: Finish::Committed,
        review,
    })
}

fn apply_dep_reasons(
    handle: &alpm::Alpm,
    fresh_deps: &[&String],
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
) {
    for name in fresh_deps {
        let outcome = handle
            .localdb()
            .pkg(name.as_str())
            .map_err(|error| anyhow::anyhow!("package {name} is not installed: {error}"))
            .and_then(|pkg| {
                pkg.set_reason(alpm::PackageReason::Depend)
                    .map_err(|error| {
                        anyhow::anyhow!("failed to mark {name} as dependency: {error}")
                    })
            });
        if let Err(error) = outcome {
            events.borrow_mut().event(InstallEvent::Log {
                level: LogLevel::Warning,
                message: format!("{error:#}"),
            });
        }
    }
}

fn ask_proceed(
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
    kind: &RunKind,
) -> anyhow::Result<bool> {
    let mapped = match kind {
        RunKind::Sync | RunKind::Upgrade => TransactionKind::Install,
        RunKind::Remove(_) => TransactionKind::Remove,
    };
    let question = Question::Proceed {
        summary: summary.clone(),
        kind: mapped,
    };
    let key = question.key();
    let asked = session.borrow_mut().ask_direct(&question)?;
    let Some(answer) = asked else {
        return Ok(false);
    };
    let Answer::Proceed = answer else {
        return Err(deny_mismatch(session, key));
    };
    Ok(true)
}

fn queue_targets(
    handle: &alpm::Alpm,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<()> {
    for target in &spec.stub_targets {
        queue_stub(handle, target)?;
        fail_on_denied(&session.borrow())?;
    }
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

fn queue_upgrade(
    handle: &alpm::Alpm,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<()> {
    queue_targets(handle, spec, session)?;
    handle
        .sync_sysupgrade(false)
        .context("failed to select upgrade candidates")?;
    Ok(())
}

fn ask_missing_removal(
    session: &Rc<RefCell<QuestionSession>>,
    missing: &[String],
) -> anyhow::Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    let question = Question::RemovePkgs {
        names: missing.to_vec(),
        kind: TransactionKind::Remove,
    };
    let key = question.key();
    let asked = session.borrow_mut().ask_direct(&question)?;
    match asked {
        Some(Answer::RemovePkgs { skip: true, .. }) => Ok(()),
        Some(Answer::RemovePkgs { .. }) => {
            anyhow::bail!("target not found: {}", missing.join(", "))
        }
        None => anyhow::bail!("target not found: {}", missing.join(", ")),
        Some(_) => Err(deny_mismatch(session, key)),
    }
}

fn ask_held_removal(session: &Rc<RefCell<QuestionSession>>, held: &[String]) -> anyhow::Result<()> {
    if held.is_empty() {
        return Ok(());
    }
    let question = Question::HoldPkgs {
        names: held.to_vec(),
    };
    let key = question.key();
    let asked = session.borrow_mut().ask_direct(&question)?;
    match asked {
        Some(Answer::HoldPkgs { proceed: true, .. }) => Ok(()),
        Some(Answer::HoldPkgs { .. }) | None => {
            anyhow::bail!("held package(s) require explicit override")
        }
        Some(_) => Err(deny_mismatch(session, key)),
    }
}

fn deny_mismatch(session: &Rc<RefCell<QuestionSession>>, key: QuestionKey) -> anyhow::Error {
    session
        .borrow_mut()
        .deny(key, "answer did not match question");
    fail_on_denied(&session.borrow()).unwrap_err()
}

fn queue_remove_targets(handle: &alpm::Alpm, targets: &[String]) -> anyhow::Result<Vec<String>> {
    let mut missing = Vec::new();
    let mut queued: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue = |pkg: &alpm::Package| -> anyhow::Result<()> {
        if queued.insert(pkg.name().to_string()) {
            handle
                .trans_remove_pkg(pkg)
                .context("failed to queue package for removal")?;
        }
        Ok(())
    };
    for target in targets {
        let name = target.strip_prefix("local/").unwrap_or(target);
        if let Ok(pkg) = handle.localdb().pkg(name) {
            queue(pkg)?;
            continue;
        }
        match handle.localdb().group(name) {
            Ok(group) => {
                for pkg in group.packages().iter() {
                    queue(pkg)?;
                }
            }
            Err(_) => missing.push(name.to_string()),
        }
    }
    Ok(missing)
}

fn queue_file(handle: &alpm::Alpm, target: &str) -> anyhow::Result<()> {
    queue_loaded(
        handle,
        target,
        crate::pacman::local_file_siglevel(handle),
        "package file",
    )
}

fn queue_stub(handle: &alpm::Alpm, target: &str) -> anyhow::Result<()> {
    queue_loaded(handle, target, alpm::SigLevel::NONE, "stub package file")
}

fn queue_loaded(
    handle: &alpm::Alpm,
    target: &str,
    siglevel: alpm::SigLevel,
    description: &str,
) -> anyhow::Result<()> {
    let loaded = handle
        .pkg_load(target, true, siglevel)
        .with_context(|| format!("failed to load {description}"))?;
    handle
        .trans_add_pkg(loaded)
        .map_err(alpm::Error::from)
        .with_context(|| format!("failed to queue {description} for installation"))?;
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
        return Err(deny_mismatch(session, key));
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

fn review(
    handle: &alpm::Alpm,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
) -> anyhow::Result<Option<crate::question::review::Review>> {
    if !spec.explore {
        return Ok(None);
    }
    Ok(Some(crate::question::review::Review {
        part1: session
            .borrow()
            .recorded()
            .into_iter()
            .map(|(q, _)| q)
            .collect(),
        part2: summary.clone(),
        generated_by: explore_stamp(handle)?,
    }))
}

fn outcome(
    handle: &alpm::Alpm,
    finish: Finish,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<RunOutcome> {
    let summary = build_summary(handle);
    Ok(RunOutcome {
        review: review(handle, spec, session, &summary)?,
        summary,
        finish,
    })
}

fn explore_stamp(handle: &alpm::Alpm) -> anyhow::Result<crate::question::review::ExploreStamp> {
    let sync_dir = std::path::Path::new(handle.dbpath()).join("sync");
    let mut dbs = std::collections::BTreeMap::new();
    for db in handle.syncdbs().iter() {
        let path = sync_dir.join(format!("{}.db", db.name()));
        let mtime = std::fs::metadata(&path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        dbs.insert(
            db.name().to_string(),
            crate::question::review::DbMark {
                packages: db.pkgs().iter().count(),
                mtime,
            },
        );
    }
    Ok(crate::question::review::ExploreStamp { dbs })
}

fn closed<T>(
    session: &Rc<RefCell<QuestionSession>>,
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    result.inspect_err(|_| {
        let _ = fail_closed(session, events);
    })
}

fn fail_on_denied(session: &QuestionSession) -> anyhow::Result<()> {
    match session.denied() {
        Some(denied) => anyhow::bail!("{denied}"),
        None => Ok(()),
    }
}

fn fail_closed(
    session: &Rc<RefCell<QuestionSession>>,
    events: &Rc<RefCell<Box<dyn InstallSink>>>,
) -> anyhow::Result<()> {
    let Some(denied) = session.borrow_mut().take_denied() else {
        return Ok(());
    };
    events.borrow_mut().event(InstallEvent::FailClosed {
        key: denied.key.clone(),
        reason: denied.reason.clone(),
    });
    anyhow::bail!("{denied}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{InstallEvent, InstallSink, LogLevel};
    use crate::question::model::{Answer, Question};
    use crate::question::source::{ExploreDefaults, FailClosed, SourceDecision};
    use crate::tx::fixtures::{
        Pkg, deny, discard, fixture, fixture_full, proceed, remove_spec, script, stop,
        summary_names, sync_spec, upgrade_spec,
    };
    use std::cell::Cell;
    use std::path::Path;

    type Events = Rc<RefCell<Vec<InstallEvent>>>;

    struct Harness {
        _dir: tempfile::TempDir,
        handle: alpm::Alpm,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = self.handle.trans_init(alpm::TransFlag::NONE);
            let _ = self.handle.trans_release();
        }
    }

    impl std::ops::Deref for Harness {
        type Target = alpm::Alpm;
        fn deref(&self) -> &alpm::Alpm {
            &self.handle
        }
    }

    impl std::ops::DerefMut for Harness {
        fn deref_mut(&mut self) -> &mut alpm::Alpm {
            &mut self.handle
        }
    }

    impl Harness {
        fn fixture(packages: &[Pkg]) -> Harness {
            let (_dir, handle) = fixture(packages);
            Harness { _dir, handle }
        }

        fn fixture_full(sync: &[(&str, Vec<Pkg>)], local: &[Pkg]) -> Harness {
            let (_dir, handle) = fixture_full(sync, local);
            Harness { _dir, handle }
        }

        fn tools() -> Harness {
            Harness::fixture(&[
                Pkg::make("a", "1.0-1", &[], &[], &["tools"]),
                Pkg::make("b", "1.0-1", &[], &[], &["tools"]),
                Pkg::make("c", "1.0-1", &[], &[], &["tools"]),
            ])
        }

        fn outcome(&mut self, spec: &RunSpec, source: Box<dyn AnswerSource>) -> RunOutcome {
            self.run(spec, source, discard()).unwrap()
        }

        fn run(
            &mut self,
            spec: &RunSpec,
            source: Box<dyn AnswerSource>,
            sink: Box<dyn InstallSink>,
        ) -> anyhow::Result<RunOutcome> {
            run(&mut self.handle, spec, source, sink)
        }

        fn run_err(&mut self, spec: &RunSpec, source: Box<dyn AnswerSource>) -> anyhow::Error {
            self.run(spec, source, discard()).unwrap_err()
        }

        fn outcome_rec(
            &mut self,
            spec: &RunSpec,
            source: Box<dyn AnswerSource>,
        ) -> (RunOutcome, Events) {
            let (sink, seen) = recorder();
            (self.run(spec, source, sink).unwrap(), seen)
        }

        fn committed(&mut self, spec: &RunSpec, source: Box<dyn AnswerSource>) -> RunOutcome {
            let outcome = self.outcome(spec, source);
            assert!(matches!(outcome.finish, Finish::Committed));
            outcome
        }
    }

    #[derive(Clone, Default)]
    struct Recorder {
        seen: Events,
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

    fn recorder() -> (Box<dyn InstallSink>, Events) {
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        (Box::new(recorder), seen)
    }

    fn install(targets: &[&str], explore: bool) -> RunSpec {
        sync_spec(targets, explore)
    }

    fn sysupgrade(explore: bool) -> RunSpec {
        upgrade_spec(&[], explore)
    }

    fn removal(targets: &[&str], explore: bool) -> RunSpec {
        remove_spec(alpm::TransFlag::NONE, explore, targets)
    }

    fn solo_repo(local_solo: bool) -> Harness {
        let local = if local_solo {
            vec![Pkg::plain("solo")]
        } else {
            vec![]
        };
        Harness::fixture_full(&[("core", vec![Pkg::plain("solo")])], &local)
    }

    fn assert_stopped(outcome: &RunOutcome) {
        assert!(matches!(outcome.finish, Finish::Stopped));
    }

    fn expect_names(outcome: &RunOutcome, names: &[&str]) {
        let expected: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        assert_eq!(summary_names(outcome), expected);
    }

    fn reviewed(outcome: &RunOutcome) -> &crate::question::review::Review {
        outcome.review.as_ref().expect("explore carries a review")
    }

    fn removes(outcome: &RunOutcome, name: &str) -> bool {
        outcome
            .summary
            .packages
            .iter()
            .any(|package| package.name == name && package.is_removal)
    }

    fn warned(seen: &Events, needle: &str) -> bool {
        seen.borrow().iter().any(|event| {
            matches!(
                event,
                InstallEvent::Log {
                    level: LogLevel::Warning,
                    message,
                } if message.contains(needle)
            )
        })
    }

    fn fail_closed_events(seen: &Events) -> Vec<(QuestionKey, String)> {
        seen.borrow()
            .iter()
            .filter_map(|event| match event {
                InstallEvent::FailClosed { key, reason } => Some((key.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }

    fn group_answer(selected: &[&str]) -> Box<dyn AnswerSource> {
        let owned: Vec<String> = selected.iter().map(|name| name.to_string()).collect();
        script(move |_| {
            SourceDecision::Answer(Answer::GroupMembers {
                selected: owned.clone(),
            })
        })
    }

    fn missing_answer(skip: bool) -> Box<dyn AnswerSource> {
        script(move |question| match question {
            Question::RemovePkgs { names, .. } => SourceDecision::Answer(Answer::RemovePkgs {
                names: names.clone(),
                skip,
            }),
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
            _ => SourceDecision::Answer(Answer::Stop),
        })
    }

    fn conflict_answer(incoming: &'static str, removable: &'static str) -> Box<dyn AnswerSource> {
        script(move |_| {
            SourceDecision::Answer(Answer::Conflict {
                incoming: incoming.to_string(),
                removable: removable.to_string(),
                remove: true,
            })
        })
    }

    fn conflict_proceed() -> Box<dyn AnswerSource> {
        script(|question| match question {
            Question::Conflict {
                incoming,
                removable,
                ..
            } => SourceDecision::Answer(Answer::Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
                remove: true,
            }),
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
            _ => SourceDecision::Answer(Answer::Stop),
        })
    }

    fn dep_spec(targets: &[&str], dep_names: &[&str]) -> RunSpec {
        RunSpec {
            targets: targets.iter().map(|target| target.to_string()).collect(),
            dep_names: dep_names.iter().map(|name| name.to_string()).collect(),
            ..sync_spec(&[], false)
        }
    }

    fn reason_of(handle: &Harness, name: &str) -> alpm::PackageReason {
        handle.localdb().pkg(name).unwrap().reason()
    }

    fn peer(dbpath: &Path) -> alpm::Alpm {
        alpm::Alpm::new("/", dbpath.to_string_lossy().as_ref()).unwrap()
    }

    fn newer(local_version: &'static str, sync_version: &'static str) -> (Vec<Pkg>, Vec<Pkg>) {
        (
            vec![Pkg::make("foo", sync_version, &[], &[], &[])],
            vec![Pkg::make("foo", local_version, &[], &[], &[])],
        )
    }

    #[test]
    fn explore_review_surfaces_matrix_questions_and_stamp() {
        let conflicting = Pkg {
            version: "2.0-1",
            conflicts: vec!["oldpkg"],
            ..Pkg::plain("newpkg")
        };
        let mut h = Harness::fixture_full(&[("core", vec![conflicting])], &[Pkg::plain("oldpkg")]);
        let outcome = h.outcome(&install(&["newpkg"], true), Box::new(ExploreDefaults));
        assert_stopped(&outcome);
        let review = reviewed(&outcome);
        assert_eq!(review.part1.len(), 1);
        assert!(matches!(review.part1[0], Question::Conflict { .. }));
        assert!(
            review
                .part2
                .packages
                .iter()
                .any(|p| p.name == "oldpkg" && p.is_removal)
        );
        let mark = review.generated_by.dbs.get("core").unwrap();
        assert_eq!(mark.packages, 1);
        assert!(mark.mtime > 0);
        assert!(h.localdb().pkg("oldpkg").is_ok());

        let providers = [
            Pkg::make("provider-one", "1.0-1", &[], &["virt"], &[]),
            Pkg::make("provider-two", "1.0-1", &[], &["virt"], &[]),
        ];
        let mut h = Harness::fixture(&providers);
        let outcome = h.outcome(&install(&["virt"], true), Box::new(ExploreDefaults));
        let review = reviewed(&outcome);
        assert!(matches!(review.part1[0], Question::SelectProvider { .. }));
        assert_eq!(review.part2.packages.len(), 1);

        let mut h = Harness::fixture(&[Pkg::plain("skipme")]);
        h.add_ignorepkg("skipme").unwrap();
        let outcome = h.outcome(&install(&["skipme"], true), Box::new(ExploreDefaults));
        let review = reviewed(&outcome);
        assert!(matches!(review.part1[0], Question::InstallIgnorepkg { .. }));
        assert!(review.part2.packages.is_empty());

        let mut h = Harness::tools();
        let outcome = h.outcome(&install(&["tools"], true), Box::new(ExploreDefaults));
        assert!(matches!(
            reviewed(&outcome).part1[0],
            Question::GroupMembers { .. }
        ));
        expect_names(&outcome, &["a", "b", "c"]);

        let mut h = Harness::fixture(&[Pkg::plain("solo")]);
        let outcome = h.outcome(&install(&["solo"], false), proceed());
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(outcome.review.is_none());
    }

    #[test]
    fn removepkgs_mismatch_fails_closed() {
        let mut h = Harness::fixture_full(&[], &[Pkg::plain("sl")]);
        let (sink, seen) = recorder();
        let error = h
            .run(
                &removal(&["ghost", "sl"], false),
                conflict_answer("a", "b"),
                sink,
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("did not match"));
        assert_eq!(
            fail_closed_events(&seen),
            vec![(
                QuestionKey::RemovePkgs {
                    names: vec!["ghost".to_string()]
                },
                "answer did not match question".to_string()
            )]
        );
        assert!(h.localdb().pkg("sl").is_ok());
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
            let spec = RunSpec {
                as_deps,
                reinstall,
                ..sync_spec(&[], explore)
            };
            assert_eq!(trans_init_flags(&spec), expected);
        }
        assert_eq!(trans_init_flags(&remove_spec(F::NONE, false, &[])), F::NONE);
        assert_eq!(
            trans_init_flags(&remove_spec(F::NONE, true, &[])),
            F::DB_ONLY | F::NO_LOCK
        );
        assert_eq!(
            trans_init_flags(&remove_spec(F::NEEDED, false, &[])),
            F::NEEDED
        );
    }

    #[test]
    fn remove_queues_local_group_members() {
        let local = [Pkg::make("member", "1.0-1", &[], &[], &["fakegrp"])];
        let mut h = Harness::fixture_full(&[], &local);
        let outcome = h.committed(&removal(&["fakegrp"], false), proceed());
        assert!(removes(&outcome, "member"));
        assert!(h.localdb().pkg("member").is_err());
    }

    #[test]
    fn remove_missing_target_bails() {
        let mut h = Harness::fixture_full(&[], &[]);
        let error = h.run_err(&removal(&["ghost"], false), proceed());
        assert!(format!("{error:#}").contains("target not found: ghost"));
    }

    #[test]
    fn nothing_to_do_when_all_missing_skipped() {
        let mut h = Harness::fixture_full(&[], &[]);
        let (outcome, seen) = h.outcome_rec(&removal(&["ghost"], false), missing_answer(true));
        assert_stopped(&outcome);
        assert!(warned(&seen, "there is nothing to do"));
    }

    #[test]
    fn explore_runs_under_foreign_lock_without_committing() {
        let mut h = Harness::fixture(&[Pkg::plain("foo")]);
        let mut holder = peer(Path::new(h.dbpath()));
        holder.trans_init(alpm::TransFlag::NONE).unwrap();
        let outcome = h.outcome(&install(&["foo"], true), deny());
        assert_stopped(&outcome);
        expect_names(&outcome, &["foo"]);
        assert!(h.localdb().pkg("foo").is_err());
        holder.trans_release().unwrap();
    }

    #[test]
    fn denied_question_aborts_with_reason() {
        let mut h = Harness::fixture(&[Pkg::plain("skipme")]);
        h.add_ignorepkg("skipme").unwrap();
        let (sink, seen) = recorder();
        let error = h
            .run(&install(&["skipme"], false), deny(), sink)
            .unwrap_err();
        assert!(format!("{error:#}").contains("denied in test"));
        assert_eq!(
            fail_closed_events(&seen),
            vec![(
                QuestionKey::InstallIgnorepkg {
                    name: "skipme".to_string()
                },
                "denied in test".to_string()
            )]
        );
    }

    #[test]
    fn unsatisfiable_dep_reports_prepare_failure() {
        let mut h = Harness::fixture(&[Pkg::make("needy", "1.0-1", &["ghost>=9"], &[], &[])]);
        let outcome = h.outcome(&install(&["needy"], false), stop());
        let Finish::PrepareFailed(PrepareFailure::Unsatisfied(missing)) = outcome.finish else {
            panic!("expected an unsatisfied prepare failure");
        };
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].depend, "ghost");
    }

    #[test]
    fn file_and_stub_targets_split_siglevel() {
        let stub_dir = tempfile::tempdir().unwrap();
        let stub =
            crate::stub_pkg::build_stub_pkg("cava-git", "0.10.4-1", stub_dir.path()).unwrap();
        let target = stub.to_string_lossy().into_owned();

        let mut h = Harness::fixture(&[]);
        h.set_local_file_siglevel(alpm::SigLevel::PACKAGE).unwrap();
        let mut stubbed = install(&[], true);
        stubbed.stub_targets = vec![target.clone()];
        let (outcome, seen) = h.outcome_rec(&stubbed, stop());
        assert_stopped(&outcome);
        expect_names(&outcome, &["cava-git"]);
        assert!(
            seen.borrow()
                .iter()
                .any(|event| matches!(event, InstallEvent::LoadingPackages))
        );

        let mut h = Harness::fixture(&[]);
        h.set_local_file_siglevel(alpm::SigLevel::PACKAGE).unwrap();
        let error = h.run_err(&install(&[&target], true), stop());
        assert!(format!("{error:#}").contains("failed to load package file"));
    }

    #[test]
    fn group_selection_expands_subset_empty_stops_and_spans_dbs() {
        let mut h = Harness::tools();
        let outcome = h.outcome(&install(&["tools"], true), group_answer(&["a", "c"]));
        assert_stopped(&outcome);
        expect_names(&outcome, &["a", "c"]);

        let mut h = Harness::tools();
        let outcome = h.outcome(&install(&["tools"], true), group_answer(&[]));
        assert_stopped(&outcome);
        assert!(outcome.summary.packages.is_empty());

        let sync = [
            ("core", vec![Pkg::make("a", "1.0-1", &[], &[], &["tools"])]),
            ("extra", vec![Pkg::make("b", "1.0-1", &[], &[], &["tools"])]),
        ];
        let mut h = Harness::fixture_full(&sync, &[]);
        let outcome = h.outcome(&install(&["tools"], true), group_answer(&["a", "b"]));
        assert_stopped(&outcome);
        expect_names(&outcome, &["a", "b"]);
    }

    #[test]
    fn mismatched_answers_fail_closed() {
        let mut h = Harness::tools();
        let (sink, seen) = recorder();
        let error = h
            .run(&install(&["tools"], false), group_answer(&["ghost"]), sink)
            .unwrap_err();
        assert!(format!("{error:#}").contains("is not offered"));
        assert_eq!(
            fail_closed_events(&seen),
            vec![(
                QuestionKey::GroupMembers {
                    group: "tools".to_string()
                },
                "group member ghost is not offered".to_string()
            )]
        );

        let mut h = Harness::tools();
        let error = h.run_err(&install(&["tools"], false), conflict_answer("a", "b"));
        assert!(format!("{error:#}").contains("did not match"));

        let mut h = Harness::fixture(&[Pkg::plain("solo")]);
        let error = h.run_err(&install(&["solo"], false), group_answer(&[]));
        assert!(format!("{error:#}").contains("Proceed"));
        assert!(h.localdb().pkg("solo").is_err());
    }

    #[test]
    fn provider_answer_resolves_by_name_across_orders() {
        let orders = [
            ["provider-one", "provider-two"],
            ["provider-two", "provider-one"],
        ];
        for order in orders {
            let providers: Vec<Pkg> = order
                .iter()
                .map(|name| Pkg::make(name, "1.0-1", &[], &["virt"], &[]))
                .collect();
            let mut h = Harness::fixture(&providers);
            let source = script(|question| match question {
                Question::SelectProvider { .. } => SourceDecision::Answer(Answer::SelectProvider {
                    name: "provider-two".to_string(),
                    repo: Some("core".to_string()),
                }),
                _ => SourceDecision::Answer(Answer::Stop),
            });
            let outcome = h.outcome(&install(&["virt"], true), source);
            assert_stopped(&outcome);
            expect_names(&outcome, &["provider-two"]);
        }
    }

    #[test]
    fn needed_skips_installed_and_reinstall_requeues() {
        let sync = [Pkg::plain("foo")];
        let local = [Pkg::plain("foo")];
        let mut h = Harness::fixture_full(&[("core", sync.to_vec())], &local);
        let outcome = h.outcome(&install(&["foo"], true), stop());
        assert_stopped(&outcome);
        assert!(outcome.summary.packages.is_empty());

        let mut h = Harness::fixture_full(&[("core", sync.to_vec())], &local);
        let reinstall = RunSpec {
            targets: vec!["foo".to_string()],
            reinstall: true,
            ..sync_spec(&[], true)
        };
        let outcome = h.outcome(&reinstall, stop());
        expect_names(&outcome, &["foo"]);
    }

    #[test]
    fn commit_marks_explicit_depend_and_as_deps_reasons() {
        let sync = vec![
            Pkg::make("app", "1.0-1", &["lib"], &[], &[]),
            Pkg::plain("lib"),
        ];
        let mut h = Harness::fixture_full(&[("core", sync)], &[]);
        h.committed(&install(&["app"], false), proceed());
        assert_eq!(reason_of(&h, "app"), alpm::PackageReason::Explicit);
        assert_eq!(reason_of(&h, "lib"), alpm::PackageReason::Depend);

        let mut h = solo_repo(false);
        let as_deps = RunSpec {
            as_deps: true,
            ..install(&["solo"], false)
        };
        h.committed(&as_deps, proceed());
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Depend);
    }

    #[test]
    fn dep_names_mark_only_fresh_installs_depend() {
        let mut h = solo_repo(false);
        h.committed(&dep_spec(&["solo"], &["solo"]), proceed());
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Depend);

        let mut h = solo_repo(true);
        let reinstall = RunSpec {
            reinstall: true,
            ..dep_spec(&["solo"], &["solo"])
        };
        h.committed(&reinstall, proceed());
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Explicit);

        let mut h = solo_repo(false);
        h.committed(&dep_spec(&["solo"], &[]), proceed());
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Explicit);
    }

    #[test]
    fn as_deps_still_marks_upgraded_package_depend() {
        let mut h = solo_repo(true);
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Explicit);
        let as_deps = RunSpec {
            as_deps: true,
            reinstall: true,
            ..dep_spec(&["solo"], &[])
        };
        h.committed(&as_deps, proceed());
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Depend);
    }

    #[test]
    fn stopped_or_unknown_deps_install_nothing_unexpected() {
        let mut h = solo_repo(false);
        let outcome = h.outcome(&dep_spec(&["solo"], &["solo"]), stop());
        assert_stopped(&outcome);
        assert!(h.localdb().pkg("solo").is_err());

        let mut h = solo_repo(false);
        let (outcome, seen) = h.outcome_rec(&dep_spec(&["solo"], &["ghost"]), proceed());
        assert!(matches!(outcome.finish, Finish::Committed));
        assert_eq!(reason_of(&h, "solo"), alpm::PackageReason::Explicit);
        assert!(warned(&seen, "ghost"));
    }

    #[test]
    fn commit_conflict_removal_and_declined_proceed_stop() {
        let conflicting = Pkg {
            version: "2.0-1",
            conflicts: vec!["oldpkg"],
            ..Pkg::plain("newpkg")
        };
        let mut h = Harness::fixture_full(&[("core", vec![conflicting])], &[Pkg::plain("oldpkg")]);
        let outcome = h.outcome(&install(&["newpkg"], false), conflict_proceed());
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(h.localdb().pkg("newpkg").is_ok());
        assert!(h.localdb().pkg("oldpkg").is_err());
        assert!(removes(&outcome, "oldpkg"));

        let mut h = solo_repo(false);
        let outcome = h.outcome(&install(&["solo"], false), stop());
        assert_stopped(&outcome);
        assert!(h.localdb().pkg("solo").is_err());
    }

    #[test]
    fn first_denial_wins_across_questions() {
        let mut h = Harness::fixture(&[Pkg::plain("aaa"), Pkg::plain("bbb")]);
        h.add_ignorepkg("aaa").unwrap();
        h.add_ignorepkg("bbb").unwrap();
        let calls = Rc::new(Cell::new(0));
        let first = Rc::new(RefCell::new(String::new()));
        let source = {
            let calls = Rc::clone(&calls);
            let first = Rc::clone(&first);
            script(move |question: &Question| {
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
            })
        };
        let error = h.run_err(&install(&["aaa", "bbb"], false), source);
        assert_eq!(calls.get(), 1);
        assert!(format!("{error:#}").contains(&*first.borrow()));
    }

    #[test]
    fn commit_surfaces_post_transaction_hook_runs() {
        let mut h = Harness::fixture(&[Pkg::plain("solo")]);
        let hookdir = tempfile::tempdir().unwrap();
        std::fs::write(
            hookdir.path().join("probe.hook"),
            "[Trigger]\nOperation = Install\nType = Package\nTarget = *\n\n[Action]\nDescription = Probing hooks\nWhen = PostTransaction\nExec = /bin/true\n",
        )
        .unwrap();
        h.set_hookdirs([hookdir.path().to_string_lossy().as_ref()].iter())
            .unwrap();
        let (outcome, seen) = h.outcome_rec(&install(&["solo"], false), proceed());
        assert!(matches!(outcome.finish, Finish::Committed));
        let hook = seen.borrow().iter().find_map(|event| match event {
            InstallEvent::HookRun {
                position,
                total,
                desc,
                ..
            } => Some((*position, *total, desc.clone())),
            _ => None,
        });
        assert_eq!(hook, Some((1, 1, Some("Probing hooks".to_string()))));
    }

    #[test]
    fn tty_event_lifecycle() {
        let mut h = Harness::fixture(&[Pkg::plain("solo")]);
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let asked = log.clone();
        let source = script(move |question| {
            if matches!(question, Question::Proceed { .. }) {
                asked.borrow_mut().push("question".to_string());
                SourceDecision::Answer(Answer::Proceed)
            } else {
                SourceDecision::Answer(Answer::Stop)
            }
        });
        let sink = Box::new(OrderSink { log: log.clone() });
        let outcome = h.run(&install(&["solo"], false), source, sink).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert_eq!(
            *log.borrow(),
            vec!["summary".to_string(), "question".to_string()]
        );

        let mut h = Harness::fixture(&[Pkg::plain("foo")]);
        let (outcome, seen) = h.outcome_rec(&install(&[], false), stop());
        assert_stopped(&outcome);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        match &seen[0] {
            InstallEvent::Log { level, message } => {
                assert_eq!(*level, LogLevel::Warning);
                assert_eq!(message, "there is nothing to do");
            }
            other => panic!("expected nothing-to-do log, got {other:?}"),
        }
    }

    #[test]
    fn upgrade_selects_newer_repo_package() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let outcome = h.committed(&sysupgrade(false), proceed());
        assert!(summary_names(&outcome).contains(&"foo".to_string()));
    }

    #[test]
    fn upgrade_idle_when_up_to_date() {
        let (sync, local) = newer("1.0-1", "1.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let (outcome, seen) = h.outcome_rec(&sysupgrade(false), proceed());
        assert_stopped(&outcome);
        assert!(outcome.summary.packages.is_empty());
        assert!(
            !warned(&seen, "there is nothing to do"),
            "upgrade idle stays silent"
        );
    }

    #[test]
    fn upgrade_skips_newer_local_with_warning() {
        let (sync, local) = newer("2.0-1", "1.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let (outcome, seen) = h.outcome_rec(&sysupgrade(false), proceed());
        assert_stopped(&outcome);
        assert!(
            warned(&seen, "is newer than"),
            "newer-local warning must surface"
        );
    }

    #[test]
    fn upgrade_drops_ignored_package_with_warning() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        h.add_ignorepkg("foo").unwrap();
        let (outcome, seen) = h.outcome_rec(&sysupgrade(false), proceed());
        assert_stopped(&outcome);
        assert!(
            warned(&seen, "ignoring package upgrade"),
            "ignored-package warning must surface"
        );
    }

    #[test]
    fn upgrade_asks_replace_question_before_proceed() {
        let replacer = Pkg {
            replaces: vec!["foo"],
            ..Pkg::make("bar", "2.0-1", &[], &[], &[])
        };
        let mut h = Harness::fixture_full(
            &[("core", vec![replacer])],
            &[Pkg::make("foo", "1.0-1", &[], &[], &[])],
        );
        let order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let source = {
            let order = Rc::clone(&order);
            script(move |question| match question {
                Question::Replace { old, new, .. } => {
                    order.borrow_mut().push("replace".to_string());
                    SourceDecision::Answer(Answer::Replace {
                        old: old.clone(),
                        new: new.clone(),
                        replace: true,
                    })
                }
                Question::Proceed { .. } => {
                    order.borrow_mut().push("proceed".to_string());
                    SourceDecision::Answer(Answer::Proceed)
                }
                _ => SourceDecision::Answer(Answer::Stop),
            })
        };
        h.committed(&sysupgrade(false), source);
        assert_eq!(
            *order.borrow(),
            vec!["replace".to_string(), "proceed".to_string()]
        );
    }

    #[test]
    fn upgrade_explore_run_returns_review() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let (outcome, seen) = h.outcome_rec(&sysupgrade(true), proceed());
        assert_stopped(&outcome);
        assert!(
            reviewed(&outcome)
                .part2
                .packages
                .iter()
                .any(|p| p.name == "foo")
        );
        assert!(
            !seen
                .borrow()
                .iter()
                .any(|event| matches!(event, InstallEvent::TransactionSummary(_))),
            "explore never emits a summary event"
        );
    }

    #[test]
    fn upgrade_queues_explicit_targets_and_candidates() {
        let sync = vec![
            Pkg::make("baz", "1.0-1", &[], &[], &[]),
            Pkg::make("foo", "2.0-1", &[], &[], &[]),
        ];
        let local = vec![Pkg::make("foo", "1.0-1", &[], &[], &[])];
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let outcome = h.committed(&upgrade_spec(&["baz"], false), proceed());
        expect_names(&outcome, &["baz", "foo"]);
    }

    #[test]
    fn upgrade_emits_start_sysupgrade_before_summary() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let (outcome, seen) = h.outcome_rec(&sysupgrade(false), proceed());
        assert!(matches!(outcome.finish, Finish::Committed));
        let kinds: Vec<&str> = seen
            .borrow()
            .iter()
            .filter_map(|event| match event {
                InstallEvent::StartSysupgrade => Some("start"),
                InstallEvent::TransactionSummary(_) => Some("summary"),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec!["start", "summary"]);
    }

    #[test]
    fn upgrade_explore_emits_no_start_sysupgrade() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let (sink, seen) = recorder();
        let outcome = h
            .run(&sysupgrade(true), Box::new(ExploreDefaults), sink)
            .unwrap();
        assert_stopped(&outcome);
        assert!(
            !seen
                .borrow()
                .iter()
                .any(|event| matches!(event, InstallEvent::StartSysupgrade))
        );
    }

    #[test]
    fn upgrade_proceed_uses_install_wording() {
        let (sync, local) = newer("1.0-1", "2.0-1");
        let mut h = Harness::fixture_full(&[("core", sync)], &local);
        let seen: Rc<Cell<Option<TransactionKind>>> = Rc::new(Cell::new(None));
        let source = {
            let seen = Rc::clone(&seen);
            script(move |question| match question {
                Question::Proceed { kind, .. } => {
                    seen.set(Some(*kind));
                    SourceDecision::Answer(Answer::Proceed)
                }
                _ => SourceDecision::Answer(Answer::Stop),
            })
        };
        h.committed(&sysupgrade(false), source);
        assert_eq!(seen.get(), Some(TransactionKind::Install));
    }
}
