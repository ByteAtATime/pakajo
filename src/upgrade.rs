use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::events::{InstallEvent, InstallSink, TransactionSummary};
use crate::install::{QuestionState, build_summary, register_callbacks};

pub fn run_repo_sysupgrade<S: InstallSink + 'static>(
    no_refresh: bool,
    sink: S,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm(&config)?;
    if !no_refresh {
        handle
            .syncdbs_mut()
            .update(false)
            .context("failed to refresh sync DBs")?;
    }
    repo_sysupgrade_into(&mut handle, sink, answerer)
}

fn repo_sysupgrade_into<S: InstallSink + 'static>(
    handle: &mut alpm::Alpm,
    sink: S,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    let qstate = Rc::new(RefCell::new(QuestionState::new(answerer)));
    register_callbacks(handle, sink.clone(), qstate.clone());
    let result = run_sysupgrade_transaction(handle, &sink, &qstate);
    let _ = handle.trans_release();
    result
}

fn run_sysupgrade_transaction<S: InstallSink>(
    handle: &mut alpm::Alpm,
    sink: &Rc<RefCell<S>>,
    qstate: &Rc<RefCell<QuestionState>>,
) -> anyhow::Result<()> {
    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;

    handle
        .sync_sysupgrade(false)
        .context("failed to populate sysupgrade targets")?;

    handle
        .trans_prepare()
        .map_err(alpm::Error::from)
        .context("failed to prepare transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    let summary: TransactionSummary = build_summary(handle);
    sink.borrow_mut()
        .event(InstallEvent::TransactionSummary(summary));

    handle
        .trans_commit()
        .map_err(alpm::Error::from)
        .context("failed to commit transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConsoleSink;
    use crate::install::setup_fake_root;

    #[test]
    #[ignore]
    fn repo_sysupgrade_into_empty() {
        let mut handle = setup_fake_root("sysupgrade_empty");
        let result = repo_sysupgrade_into(
            &mut handle,
            ConsoleSink::new(),
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        result.expect("sysupgrade with nothing to do should succeed");
    }
}
