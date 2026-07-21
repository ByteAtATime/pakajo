use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::events::{InstallEvent, InstallSink, TransactionSummary};
use crate::install::{QuestionState, build_summary, register_callbacks};

pub fn run_repo_sysupgrade<S: InstallSink + 'static>(
    no_refresh: bool,
    extra_ignores: &[String],
    sink: S,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm(&config)?;
    apply_ignores(&mut handle, &config, extra_ignores);
    if !no_refresh {
        handle
            .syncdbs_mut()
            .update(false)
            .context("failed to refresh sync DBs")?;
    }
    repo_sysupgrade_into(&mut handle, sink, answerer)
}

fn apply_ignores(handle: &mut alpm::Alpm, config: &pacmanconf::Config, extra: &[String]) {
    for name in &config.ignore_pkg {
        let _ = handle.add_ignorepkg(name.as_str());
    }
    for group in &config.ignore_group {
        let _ = handle.add_ignoregroup(group.as_str());
    }
    for name in extra {
        let _ = handle.add_ignorepkg(name.as_str());
    }
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
    use std::collections::HashSet;

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

    #[test]
    fn apply_ignores_propagates_to_handle() {
        let base = std::env::temp_dir().join("pakajo_upgrade_ignores_test");
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().into_owned(),
            db.to_string_lossy().into_owned(),
        )
        .unwrap();

        let mut config = pacmanconf::Config::default();
        config.ignore_pkg = vec!["foo".to_string()];
        config.ignore_group = vec!["bar".to_string()];

        apply_ignores(&mut handle, &config, &["baz".to_string()]);

        let pkgs: HashSet<String> = handle.ignorepkgs().iter().map(|s| s.to_string()).collect();
        let groups: HashSet<String> = handle
            .ignoregroups()
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert!(
            pkgs.contains("foo"),
            "ignore_pkg from config must be set: {pkgs:?}"
        );
        assert!(pkgs.contains("baz"), "extra ignore must be set: {pkgs:?}");
        assert!(
            groups.contains("bar"),
            "ignore_group from config must be set: {groups:?}"
        );
    }
}
