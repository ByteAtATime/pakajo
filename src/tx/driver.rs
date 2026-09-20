use anyhow::Context;

use crate::events::TransactionSummary;
use crate::pacman::lock::{
    LOCK_POLL_INTERVAL, cleanup_on_signal, during_commit, finish_transaction, lock_retry,
};
use crate::question::source::AnswerSource;
use crate::tx::convert::{PrepareFailure, build_summary, extract_prepare_failure};
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
) -> anyhow::Result<RunOutcome> {
    cleanup_on_signal(handle);
    lock_retry(
        || handle.trans_init(trans_init_flags(spec.explore, spec.as_deps, spec.reinstall)),
        || {},
        LOCK_POLL_INTERVAL,
    )
    .context("failed to initialize transaction")?;
    let outcome = drive(handle, spec, source);
    finish_transaction(handle);
    outcome
}

fn drive(
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    source: Box<dyn AnswerSource>,
) -> anyhow::Result<RunOutcome> {
    let session = QuestionSession::attach(handle, source);
    if !spec.targets.is_empty() {
        queue_targets(handle, spec)?;
    }
    if handle.trans_add().is_empty() {
        fail_on_denied(&session.borrow())?;
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
    if spec.explore {
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

fn queue_targets(handle: &alpm::Alpm, spec: &RunSpec) -> anyhow::Result<()> {
    let resolved = resolve_targets(handle, &spec.targets)?;
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
        Some(denied) => anyhow::bail!("aborted: {:?}: {}", denied.key, denied.reason),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::model::{Answer, Question, QuestionKey};
    use crate::question::source::{FailClosed, SourceDecision};
    use std::fs::File;
    use std::path::Path;

    struct Stub {
        abort: bool,
    }

    impl AnswerSource for Stub {
        fn answer(&self, _question: &Question) -> SourceDecision {
            if self.abort {
                SourceDecision::Abort(FailClosed {
                    key: QuestionKey::Proceed,
                    reason: "denied in test".to_string(),
                })
            } else {
                SourceDecision::Answer(Answer::Stop)
            }
        }
    }

    fn deny() -> Box<dyn AnswerSource> {
        Box::new(Stub { abort: true })
    }

    fn stop() -> Box<dyn AnswerSource> {
        Box::new(Stub { abort: false })
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

    fn desc(name: &str, version: &str, depends: &[&str]) -> Vec<u8> {
        let mut out = format!(
            "%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}-{version}-x86_64.pkg.tar.zst\n\n"
        );
        if !depends.is_empty() {
            out.push_str("%DEPENDS%\n");
            for depend in depends {
                out.push_str(depend);
                out.push('\n');
            }
            out.push('\n');
        }
        out.into_bytes()
    }

    fn write_syncdb(dbpath: &Path, repo: &str, packages: &[(&str, &str, &[&str])]) {
        let file = File::create(dbpath.join("sync").join(format!("{repo}.db"))).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, version, depends) in packages {
            let content = desc(name, version, depends);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{name}-{version}/desc"),
                    content.as_slice(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
    }

    fn fixture(packages: &[(&str, &str, &[&str])]) -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sync")).unwrap();
        let handle = alpm::Alpm::new("/", dir.path().to_string_lossy().as_ref()).unwrap();
        write_syncdb(dir.path(), "core", packages);
        handle
            .register_syncdb("core", alpm::SigLevel::NONE)
            .unwrap();
        (dir, handle)
    }

    fn peer(dbpath: &Path) -> alpm::Alpm {
        alpm::Alpm::new("/", dbpath.to_string_lossy().as_ref()).unwrap()
    }

    #[test]
    fn init_flags_derive_from_spec() {
        use alpm::TransFlag as F;
        assert_eq!(trans_init_flags(false, false, false), F::NEEDED);
        assert_eq!(trans_init_flags(false, false, true), F::NONE);
        assert_eq!(
            trans_init_flags(false, true, false),
            F::NEEDED | F::ALL_DEPS
        );
        assert_eq!(
            trans_init_flags(true, false, false),
            F::NEEDED | F::DB_ONLY | F::NO_LOCK
        );
    }

    #[test]
    fn explore_runs_under_foreign_lock_without_committing() {
        let (_dir, mut handle) = fixture(&[("foo", "1.0-1", &[])]);
        let mut holder = peer(Path::new(handle.dbpath()));
        holder.trans_init(alpm::TransFlag::NONE).unwrap();
        let outcome = run(&mut handle, &spec(&["foo"], true), deny()).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        assert_eq!(outcome.summary.packages.len(), 1);
        assert_eq!(outcome.summary.packages[0].name, "foo");
        assert!(handle.localdb().pkg("foo").is_err());
        holder.trans_release().unwrap();
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }

    #[test]
    fn denied_question_aborts_with_reason() {
        let (_dir, mut handle) = fixture(&[("skipme", "1.0-1", &[])]);
        handle.add_ignorepkg("skipme").unwrap();
        let error = run(&mut handle, &spec(&["skipme"], false), deny()).unwrap_err();
        assert!(format!("{error:#}").contains("denied in test"));
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }

    #[test]
    fn unsatisfiable_dep_reports_prepare_failure() {
        let (_dir, mut handle) = fixture(&[("needy", "1.0-1", &["ghost>=9"])]);
        let outcome = run(&mut handle, &spec(&["needy"], false), stop()).unwrap();
        let Finish::PrepareFailed(PrepareFailure::Unsatisfied(missing)) = outcome.finish else {
            panic!("expected an unsatisfied prepare failure");
        };
        assert_eq!(missing.len(), 1);
        assert!(missing[0].depend.contains("ghost"));
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }
}
