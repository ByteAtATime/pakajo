use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context, anyhow, bail};

use crate::events::{InstallEvent, InstallSink, LogLevel};
use crate::holdpkg::HoldGate;
use crate::install::{QuestionState, register_callbacks};
use crate::pacman;

pub fn run_remove<S: InstallSink + 'static, F: FnOnce() -> bool, G: FnOnce() -> bool>(
    targets: &[String],
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
    approved_held: &[String],
    confirm_hold: G,
) -> anyhow::Result<()> {
    let hold_patterns = pacman::config()?.hold_pkg;
    let gate = HoldGate::new(&hold_patterns, approved_held);
    let mut handle = pacman::handle()?;
    remove_into(
        &mut handle,
        targets,
        sink,
        confirm,
        answerer,
        &gate,
        confirm_hold,
    )
}

fn remove_into<S: InstallSink + 'static, F: FnOnce() -> bool, G: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[String],
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
    gate: &HoldGate,
    confirm_hold: G,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    let qstate = Rc::new(RefCell::new(QuestionState::new(answerer)));
    register_callbacks(handle, sink.clone(), qstate.clone());
    let result =
        run_remove_transaction(handle, targets, &sink, &qstate, confirm, gate, confirm_hold);
    pacman::lock::finish_transaction(handle);
    result
}

fn run_remove_transaction<S: InstallSink, F: FnOnce() -> bool, G: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[String],
    sink: &Rc<RefCell<S>>,
    qstate: &Rc<RefCell<QuestionState>>,
    confirm: F,
    gate: &HoldGate,
    confirm_hold: G,
) -> anyhow::Result<()> {
    pacman::lock::cleanup_on_signal(handle);

    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;

    for name in targets {
        let pkg = handle
            .localdb()
            .pkg(name.as_str())
            .map_err(|_| anyhow!("package '{name}' is not installed"))?;
        handle
            .trans_remove_pkg(pkg)
            .context("failed to queue package for removal")?;
    }

    if let Err(prepare_err) = handle.trans_prepare() {
        return Err(classify_prepare_error(prepare_err));
    }

    if qstate.borrow().deny_flag {
        bail!("aborted: {}", qstate.borrow().detail);
    }

    enforce_hold_gate(handle, sink, gate, confirm_hold)?;

    let summary = crate::install::build_summary(handle);
    sink.borrow_mut()
        .event(InstallEvent::TransactionSummary(summary));

    if !confirm() {
        return Ok(());
    }

    let commit = pacman::lock::during_commit(|| handle.trans_commit());
    commit.context("failed to commit transaction")?;
    if qstate.borrow().deny_flag {
        bail!("aborted: {}", qstate.borrow().detail);
    }

    Ok(())
}

fn enforce_hold_gate<S: InstallSink, G: FnOnce() -> bool>(
    handle: &alpm::Alpm,
    sink: &Rc<RefCell<S>>,
    gate: &HoldGate,
    confirm_hold: G,
) -> anyhow::Result<()> {
    let names: Vec<String> = handle
        .trans_remove()
        .iter()
        .map(|pkg| pkg.name().to_string())
        .collect();
    let held = gate.held(&names);
    for name in &held {
        sink.borrow_mut().event(InstallEvent::Log {
            level: LogLevel::Warning,
            message: format!("{name} is designated as a HoldPkg."),
        });
    }
    if gate.pending(&names).is_empty() || confirm_hold() {
        return Ok(());
    }
    bail!("held package(s) require explicit override")
}

fn classify_prepare_error(err: alpm::PrepareError) -> anyhow::Error {
    match err.data() {
        Some(alpm::PrepareData::UnsatisfiedDeps(list)) => {
            let detail = list
                .iter()
                .map(|d| {
                    let target = d.target();
                    let depend = d.depend().name();
                    format!("{depend} is required by {target}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            anyhow!("unsatisfied dependencies: {detail}")
        }
        Some(other) => anyhow!("trans_prepare failed: {other:?}"),
        None => anyhow!("trans_prepare failed: {}", err.error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answerer::DenyAllAnswerer;
    use crate::cli::ConsoleSink;
    use crate::install::{InstallTarget, install_into, setup_fake_root};

    #[test]
    #[ignore]
    fn test_remove() {
        let mut handle = setup_fake_root("remove");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
        )
        .expect("sl should install first");
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl must be installed before remove"
        );

        remove_into(
            &mut handle,
            &["sl".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&[], &[]),
            || true,
        )
        .expect("remove should succeed");
        assert!(handle.localdb().pkg("sl").is_err(), "sl must be removed");
    }

    #[test]
    #[ignore]
    fn test_remove_multiple() {
        let mut handle = setup_fake_root("remove_multiple");
        install_into(
            &mut handle,
            &[
                InstallTarget::Repo("sl".to_string()),
                InstallTarget::Repo("figlet".to_string()),
            ],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
        )
        .expect("sl and figlet should install first");
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl must be installed before remove"
        );
        assert!(
            handle.localdb().pkg("figlet").is_ok(),
            "figlet must be installed before remove"
        );

        remove_into(
            &mut handle,
            &["sl".to_string(), "figlet".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&[], &[]),
            || true,
        )
        .expect("remove should succeed");
        assert!(handle.localdb().pkg("sl").is_err(), "sl must be removed");
        assert!(
            handle.localdb().pkg("figlet").is_err(),
            "figlet must be removed"
        );
    }

    #[test]
    #[ignore]
    fn test_remove_uninstalled_errors() {
        let mut handle = setup_fake_root("remove_uninstalled");
        let result = remove_into(
            &mut handle,
            &["definitely-not-installed".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&[], &[]),
            || true,
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("not installed"),
            "expected 'not installed' in error, got: {err}"
        );
    }

    #[test]
    #[ignore]
    fn test_remove_needed_by_other_errors() {
        let mut handle = setup_fake_root("remove_needed");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("vlc".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
        )
        .expect("vlc should install (pulls ffmpeg)");
        assert!(
            handle.localdb().pkg("ffmpeg").is_ok(),
            "ffmpeg must be installed as a vlc dep"
        );

        let result = remove_into(
            &mut handle,
            &["ffmpeg".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&[], &[]),
            || true,
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.to_lowercase().contains("unsatisfied") || err.contains("vlc"),
            "expected 'unsatisfied' or 'vlc' in error, got: {err}"
        );
    }

    #[test]
    #[ignore]
    fn test_remove_held_declined_keeps_package() {
        let mut handle = setup_fake_root("remove_held_declined");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
        )
        .expect("sl should install first");
        let patterns = ["sl".to_string()];
        let result = remove_into(
            &mut handle,
            &["sl".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&patterns, &[]),
            || false,
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("held package(s) require explicit override"),
            "expected held error, got: {err}"
        );
        assert!(
            handle.localdb().pkg("sl").is_ok(),
            "sl must remain installed after declined hold gate"
        );
    }

    #[test]
    #[ignore]
    fn test_remove_held_prompt_accept_removes_package() {
        let mut handle = setup_fake_root("remove_held_prompt_accept");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
        )
        .expect("sl should install first");
        let patterns = ["sl".to_string()];
        remove_into(
            &mut handle,
            &["sl".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(DenyAllAnswerer),
            &HoldGate::new(&patterns, &[]),
            || true,
        )
        .expect("prompt-accepted held remove should succeed");
        assert!(handle.localdb().pkg("sl").is_err(), "sl must be removed");
    }
}
