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
    match &spec.kind {
        RunKind::Sync => {
            let mut flags = alpm::TransFlag::NONE;
            if !spec.reinstall {
                flags |= alpm::TransFlag::NEEDED;
            }
            if spec.as_deps {
                flags |= alpm::TransFlag::ALL_DEPS;
            }
            if spec.explore {
                flags |= alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK;
            }
            flags
        }
        RunKind::Remove(remove) => {
            let mut flags = remove.flags;
            if spec.explore {
                flags |= alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK;
            }
            flags
        }
    }
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
        || handle.trans_init(trans_init_flags(spec)),
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
    if matches!(spec.kind, RunKind::Sync)
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
        RunKind::Remove(_) => queue_remove_targets(handle, &spec.targets),
    };
    let missing = match queue_result {
        Ok(missing) => missing,
        Err(error) => {
            fail_closed(&session, events)?;
            return Err(error);
        }
    };
    if let Err(error) = ask_missing_removal(&session, &missing) {
        fail_closed(&session, events)?;
        return Err(error);
    }
    let nothing_queued = match &spec.kind {
        RunKind::Sync => handle.trans_add().is_empty(),
        RunKind::Remove(_) => handle.trans_remove().is_empty(),
    };
    if nothing_queued {
        fail_closed(&session, events)?;
        if !spec.explore {
            events.borrow_mut().event(InstallEvent::Log {
                level: LogLevel::Warning,
                message: "there is nothing to do".to_string(),
            });
        }
        return outcome(handle, Finish::Stopped, spec, &session);
    }
    let failure = match handle.trans_prepare() {
        Ok(()) => None,
        Err(error) => Some(extract_prepare_failure(error)),
    };
    if let Some(failure) = failure {
        fail_closed(&session, events)?;
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
        if let Err(error) = ask_held_removal(&session, &held) {
            fail_closed(&session, events)?;
            return Err(error);
        }
    }
    let summary = build_summary(handle);
    if !spec.explore {
        events
            .borrow_mut()
            .event(InstallEvent::TransactionSummary(summary.clone()));
    }
    let review = maybe_review(handle, spec, &session, &summary)?;
    let commit = !spec.explore
        && match ask_proceed(&session, &summary, &spec.kind) {
            Ok(proceed) => proceed,
            Err(error) => {
                fail_closed(&session, events)?;
                return Err(error);
            }
        };
    if !commit {
        return Ok(RunOutcome {
            summary,
            finish: Finish::Stopped,
            review,
        });
    }
    during_commit(|| handle.trans_commit()).context("failed to commit transaction")?;
    fail_closed(&session, events)?;
    Ok(RunOutcome {
        summary,
        finish: Finish::Committed,
        review,
    })
}

fn ask_proceed(
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
    kind: &RunKind,
) -> anyhow::Result<bool> {
    let mapped = match kind {
        RunKind::Sync => TransactionKind::Install,
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
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for target in targets {
        let name = target.strip_prefix("local/").unwrap_or(target);
        if let Ok(pkg) = handle.localdb().pkg(name) {
            if seen.insert(name.to_string()) {
                handle
                    .trans_remove_pkg(pkg)
                    .context("failed to queue package for removal")?;
            }
            continue;
        }
        if let Ok(group) = handle.localdb().group(name) {
            for pkg in group.packages().iter() {
                if seen.insert(pkg.name().to_string()) {
                    handle
                        .trans_remove_pkg(pkg)
                        .context("failed to queue package for removal")?;
                }
            }
            continue;
        }
        missing.push(name.to_string());
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

fn explore_review(
    handle: &alpm::Alpm,
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
) -> anyhow::Result<crate::question::review::Review> {
    Ok(crate::question::review::Review {
        part1: session
            .borrow()
            .recorded()
            .into_iter()
            .map(|(q, _)| q)
            .collect(),
        part2: summary.clone(),
        generated_by: explore_stamp(handle)?,
    })
}

fn outcome(
    handle: &alpm::Alpm,
    finish: Finish,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
) -> anyhow::Result<RunOutcome> {
    let summary = build_summary(handle);
    Ok(RunOutcome {
        review: maybe_review(handle, spec, session, &summary)?,
        summary,
        finish,
    })
}

fn maybe_review(
    handle: &alpm::Alpm,
    spec: &RunSpec,
    session: &Rc<RefCell<QuestionSession>>,
    summary: &TransactionSummary,
) -> anyhow::Result<Option<crate::question::review::Review>> {
    if !spec.explore {
        return Ok(None);
    }
    explore_review(handle, session, summary).map(Some)
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
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
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
            stub_targets: Vec::new(),
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
                    &package.provides,
                    &package.conflicts,
                    &package.groups,
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
    fn explore_review_surfaces_matrix_questions_and_stamp() {
        use crate::question::source::ExploreDefaults;

        let conflicting = Pkg {
            version: "2.0-1",
            conflicts: vec!["oldpkg"],
            ..plain("newpkg")
        };
        let (_dir, mut handle) = fixture_full(&[("core", vec![conflicting])], &[plain("oldpkg")]);
        let outcome = run(
            &mut handle,
            &spec(&["newpkg"], true),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert_eq!(review.part1.len(), 1);
        assert!(matches!(review.part1[0], Question::Conflict { .. }));
        assert!(
            review
                .part2
                .packages
                .iter()
                .any(|package| package.name == "oldpkg" && package.is_removal)
        );
        let mark = review.generated_by.dbs.get("core").unwrap();
        assert_eq!(mark.packages, 1);
        assert!(mark.mtime > 0);
        assert!(handle.localdb().pkg("oldpkg").is_ok());
        release(&mut handle);

        let providers = vec![
            make("provider-one", "1.0-1", &[], &["virt"], &[]),
            make("provider-two", "1.0-1", &[], &["virt"], &[]),
        ];
        let (_dir, mut handle) = fixture(&providers);
        let outcome = run(
            &mut handle,
            &spec(&["virt"], true),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert!(matches!(review.part1[0], Question::SelectProvider { .. }));
        assert_eq!(review.part2.packages.len(), 1);
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[plain("skipme")]);
        handle.add_ignorepkg("skipme").unwrap();
        let outcome = run(
            &mut handle,
            &spec(&["skipme"], true),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert!(matches!(review.part1[0], Question::InstallIgnorepkg { .. }));
        assert!(review.part2.packages.is_empty());
        release(&mut handle);

        let (_dir, mut handle) = tools_fixture();
        let outcome = run(
            &mut handle,
            &spec(&["tools"], true),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert!(matches!(review.part1[0], Question::GroupMembers { .. }));
        assert_eq!(
            summary_names(&outcome),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[plain("solo")]);
        let outcome = run(&mut handle, &spec(&["solo"], false), proceed(), discard()).unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(outcome.review.is_none());
        release(&mut handle);
    }

    fn sync_spec(explore: bool, as_deps: bool, reinstall: bool) -> RunSpec {
        RunSpec {
            kind: RunKind::Sync,
            targets: Vec::new(),
            stub_targets: Vec::new(),
            explore,
            as_deps,
            reinstall,
        }
    }

    fn remove_spec(flags: alpm::TransFlag, explore: bool, targets: &[&str]) -> RunSpec {
        RunSpec {
            kind: RunKind::Remove(RemoveSpec {
                flags,
                holds: Vec::new(),
            }),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            stub_targets: Vec::new(),
            explore,
            as_deps: false,
            reinstall: false,
        }
    }

    fn hold_spec(holds: &[&str], explore: bool, targets: &[&str]) -> RunSpec {
        RunSpec {
            kind: RunKind::Remove(RemoveSpec {
                flags: alpm::TransFlag::NONE,
                holds: holds.iter().map(|hold| hold.to_string()).collect(),
            }),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            stub_targets: Vec::new(),
            explore,
            as_deps: false,
            reinstall: false,
        }
    }

    fn hold_answer(proceed: bool) -> Box<dyn AnswerSource> {
        script(move |question| match question {
            Question::HoldPkgs { names } => SourceDecision::Answer(Answer::HoldPkgs {
                names: names.clone(),
                proceed,
            }),
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
            _ => SourceDecision::Answer(Answer::Stop),
        })
    }

    #[test]
    fn hold_decline_aborts_without_removing() {
        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let error = run(
            &mut handle,
            &hold_spec(&["sl"], false, &["sl"]),
            hold_answer(false),
            discard(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("held package(s) require explicit override"));
        assert!(handle.localdb().pkg("sl").is_ok());
        release(&mut handle);
    }

    #[test]
    fn hold_accept_commits_removal() {
        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let outcome = run(
            &mut handle,
            &hold_spec(&["sl"], false, &["sl"]),
            hold_answer(true),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(handle.localdb().pkg("sl").is_err());
        release(&mut handle);
    }

    #[test]
    fn explore_records_hold_question() {
        use crate::question::source::ExploreDefaults;

        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let outcome = run(
            &mut handle,
            &hold_spec(&["sl"], true, &["sl"]),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert!(review.part1.iter().any(|question| matches!(
            question,
            Question::HoldPkgs { names } if names == &vec!["sl".to_string()]
        )));
        assert!(handle.localdb().pkg("sl").is_ok());
        release(&mut handle);
    }

    #[test]
    fn removepkgs_mismatch_fails_closed() {
        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let mismatched = script(|_| {
            SourceDecision::Answer(Answer::Conflict {
                incoming: "a".to_string(),
                removable: "b".to_string(),
                remove: true,
            })
        });
        let error = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["ghost", "sl"]),
            mismatched,
            Box::new(recorder),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("did not match"));
        assert_eq!(
            fail_closed_events(&seen.borrow()),
            vec![(
                crate::question::model::QuestionKey::RemovePkgs {
                    names: vec!["ghost".to_string()]
                },
                "answer did not match question".to_string()
            )]
        );
        assert!(handle.localdb().pkg("sl").is_ok());
        release(&mut handle);
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
            assert_eq!(
                trans_init_flags(&sync_spec(explore, as_deps, reinstall)),
                expected
            );
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
        let local = [make("member", "1.0-1", &[], &[], &["fakegrp"])];
        let (_dir, mut handle) = fixture_full(&[], &local);
        let outcome = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["fakegrp"]),
            proceed(),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(
            outcome
                .summary
                .packages
                .iter()
                .any(|package| package.name == "member" && package.is_removal)
        );
        assert!(handle.localdb().pkg("member").is_err());
        release(&mut handle);
    }

    #[test]
    fn remove_missing_target_bails() {
        let (_dir, mut handle) = fixture_full(&[], &[]);
        let error = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["ghost"]),
            proceed(),
            discard(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("target not found: ghost"));
        release(&mut handle);
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

    #[test]
    fn missing_target_skip_continues() {
        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let outcome = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["ghost", "sl"]),
            missing_answer(true),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Committed));
        assert!(handle.localdb().pkg("sl").is_err());
        assert!(!summary_names(&outcome).contains(&"ghost".to_string()));
        release(&mut handle);
    }

    #[test]
    fn missing_target_decline_aborts() {
        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let error = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["ghost", "sl"]),
            missing_answer(false),
            discard(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("target not found: ghost"));
        assert!(handle.localdb().pkg("sl").is_ok());
        release(&mut handle);
    }

    #[test]
    fn nothing_to_do_when_all_missing_skipped() {
        let (_dir, mut handle) = fixture_full(&[], &[]);
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let outcome = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, false, &["ghost"]),
            missing_answer(true),
            Box::new(recorder),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert!(seen.borrow().iter().any(|event| matches!(
            event,
            InstallEvent::Log {
                level: LogLevel::Warning,
                message,
            } if message == "there is nothing to do"
        )));
        release(&mut handle);
    }

    #[test]
    fn explore_records_missing_question() {
        use crate::question::source::ExploreDefaults;

        let (_dir, mut handle) = fixture_full(&[], &[plain("sl")]);
        let outcome = run(
            &mut handle,
            &remove_spec(alpm::TransFlag::NONE, true, &["ghost", "sl"]),
            Box::new(ExploreDefaults),
            discard(),
        )
        .unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let review = outcome.review.as_ref().expect("explore carries a review");
        assert!(review.part1.iter().any(|question| matches!(
            question,
            Question::RemovePkgs { names, kind: TransactionKind::Remove }
            if names == &vec!["ghost".to_string()]
        )));
        assert!(summary_names(&outcome).contains(&"sl".to_string()));
        assert!(handle.localdb().pkg("sl").is_ok());
        release(&mut handle);
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
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let error = run(
            &mut handle,
            &spec(&["skipme"], false),
            deny(),
            Box::new(recorder),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("denied in test"));
        assert_eq!(
            fail_closed_events(&seen.borrow()),
            vec![(
                crate::question::model::QuestionKey::InstallIgnorepkg {
                    name: "skipme".to_string()
                },
                "denied in test".to_string()
            )]
        );
        release(&mut handle);
    }

    fn fail_closed_events(
        seen: &[InstallEvent],
    ) -> Vec<(crate::question::model::QuestionKey, String)> {
        seen.iter()
            .filter_map(|event| match event {
                InstallEvent::FailClosed { key, reason } => Some((key.clone(), reason.clone())),
                _ => None,
            })
            .collect()
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
    fn file_and_stub_targets_split_siglevel() {
        let stub_dir = tempfile::tempdir().unwrap();
        let stub =
            crate::stub_pkg::build_stub_pkg("cava-git", "0.10.4-1", stub_dir.path()).unwrap();
        let target = stub.to_string_lossy().into_owned();
        let strict = |handle: &mut alpm::Alpm| {
            handle
                .set_local_file_siglevel(alpm::SigLevel::PACKAGE)
                .unwrap()
        };

        let (_dir, mut handle) = fixture(&[]);
        strict(&mut handle);
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let mut stubbed = spec(&[], true);
        stubbed.stub_targets = vec![target.clone()];
        let outcome = run(&mut handle, &stubbed, stop(), Box::new(recorder)).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(summary_names(&outcome), vec!["cava-git".to_string()]);
        assert!(
            seen.borrow()
                .iter()
                .any(|event| matches!(event, InstallEvent::LoadingPackages))
        );
        release(&mut handle);

        let (_dir, mut handle) = fixture(&[]);
        strict(&mut handle);
        let error = run(&mut handle, &spec(&[&target], true), stop(), discard()).unwrap_err();
        assert!(format!("{error:#}").contains("failed to load package file"));
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
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let error = run(
            &mut handle,
            &spec(&["tools"], false),
            group_answer(&["ghost"]),
            Box::new(recorder),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("is not offered"));
        assert_eq!(
            fail_closed_events(&seen.borrow()),
            vec![(
                crate::question::model::QuestionKey::GroupMembers {
                    group: "tools".to_string()
                },
                "group member ghost is not offered".to_string()
            )]
        );
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
            Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
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
    fn commit_surfaces_post_transaction_hook_runs() {
        let (_dir, mut handle) = fixture(&[plain("solo")]);
        let hookdir = tempfile::tempdir().unwrap();
        std::fs::write(
            hookdir.path().join("probe.hook"),
            "[Trigger]\nOperation = Install\nType = Package\nTarget = *\n\n[Action]\nDescription = Probing hooks\nWhen = PostTransaction\nExec = /bin/true\n",
        )
        .unwrap();
        handle
            .set_hookdirs([hookdir.path().to_string_lossy().as_ref()].iter())
            .unwrap();
        let recorder = Recorder::default();
        let seen = recorder.seen.clone();
        let outcome = run(
            &mut handle,
            &spec(&["solo"], false),
            proceed(),
            Box::new(recorder),
        )
        .unwrap();
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
        release(&mut handle);
    }

    #[test]
    fn tty_event_lifecycle() {
        let (_dir, mut handle) = fixture(&[plain("solo")]);
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
                assert_eq!(message, "there is nothing to do");
            }
            other => panic!("expected nothing-to-do log, got {other:?}"),
        }
        release(&mut handle);
    }
}
