use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context, anyhow};

use crate::events::{InstallSink, SummaryPackage, TransactionSummary};
use crate::install::{QuestionState, register_callbacks};

pub fn run_remove<S: InstallSink + 'static, F: FnOnce() -> bool>(
    targets: &[String],
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm(&config)?;
    remove_into(&mut handle, targets, sink, confirm, answerer)
}

fn remove_into<S: InstallSink + 'static, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[String],
    sink: S,
    confirm: F,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    let qstate = Rc::new(RefCell::new(QuestionState::new(answerer)));
    register_callbacks(handle, sink.clone(), qstate.clone());
    let result = run_remove_transaction(handle, targets, &sink, &qstate, confirm);
    let _ = handle.trans_release();
    result
}

fn run_remove_transaction<S: InstallSink, F: FnOnce() -> bool>(
    handle: &mut alpm::Alpm,
    targets: &[String],
    sink: &Rc<RefCell<S>>,
    qstate: &Rc<RefCell<QuestionState>>,
    confirm: F,
) -> anyhow::Result<()> {
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
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    let summary = build_remove_summary(handle);
    sink.borrow_mut()
        .event(crate::events::InstallEvent::TransactionSummary(summary));

    if !confirm() {
        return Ok(());
    }

    handle
        .trans_commit()
        .context("failed to commit transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    Ok(())
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

fn build_remove_summary(handle: &alpm::Alpm) -> TransactionSummary {
    let mut packages = Vec::new();
    let mut total_removed_size = 0;
    for pkg in handle.trans_remove().iter() {
        let name = pkg.name().to_string();
        let old_version = pkg.version().to_string();
        let installed_size = pkg.isize();
        total_removed_size += installed_size;
        packages.push(SummaryPackage {
            repository: None,
            new_version: String::new(),
            name,
            old_version: Some(old_version),
            download_size: 0,
            installed_size,
            old_installed_size: 0,
            is_removal: true,
        });
    }
    TransactionSummary {
        packages,
        total_download_size: 0,
        total_installed_size: 0,
        total_removed_size,
    }
}

pub(crate) fn run_remove_process(
    exe: std::path::PathBuf,
    names: Vec<String>,
    mut tx: futures::channel::mpsc::Sender<crate::install::StreamItem>,
) {
    use std::io::BufRead as _;
    use std::process::{Command, Stdio};

    let mut send_event = |mut item: crate::install::StreamItem| loop {
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
    cmd.arg("remove").arg("--json");
    for name in &names {
        cmd.arg(name);
    }
    let outcome = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::install::ChildOutcome::NotFound
        }
        Err(error) => crate::install::ChildOutcome::Failed(error.to_string()),
        Ok(mut child) => {
            let stdout = child.stdout.take().expect("piped");
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if let Ok(ev) = serde_json::from_str::<crate::events::InstallEvent>(&l) {
                            send_event(crate::install::StreamItem::Event(ev));
                        }
                    }
                    Err(_) => break,
                }
            }
            crate::install::map_outcome(child.wait())
        }
    };
    send_event(crate::install::StreamItem::Done(outcome));
}

pub(crate) fn spawn_remove_child(targets: &[String]) -> anyhow::Result<std::process::Child> {
    use std::process::Stdio;
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = crate::cli::escalation_command(&exe.to_string_lossy());
    cmd.arg("remove").arg("--json");
    for target in targets {
        cmd.arg(target);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn().context("failed to spawn remove child")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConsoleSink;
    use crate::install::{InstallTarget, install_into};

    #[test]
    #[ignore]
    fn test_remove() {
        let mut handle = crate::install::setup_fake_root("remove");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("sl".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
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
            Box::new(crate::answerer::DenyAllAnswerer),
        )
        .expect("remove should succeed");
        assert!(handle.localdb().pkg("sl").is_err(), "sl must be removed");
    }

    #[test]
    #[ignore]
    fn test_remove_multiple() {
        let mut handle = crate::install::setup_fake_root("remove_multiple");
        install_into(
            &mut handle,
            &[
                InstallTarget::Repo("sl".to_string()),
                InstallTarget::Repo("figlet".to_string()),
            ],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
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
            Box::new(crate::answerer::DenyAllAnswerer),
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
        let mut handle = crate::install::setup_fake_root("remove_uninstalled");
        let result = remove_into(
            &mut handle,
            &["definitely-not-installed".to_string()],
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
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
        let mut handle = crate::install::setup_fake_root("remove_needed");
        install_into(
            &mut handle,
            &[InstallTarget::Repo("vlc".to_string())],
            false,
            ConsoleSink::new(),
            || true,
            Box::new(crate::answerer::DenyAllAnswerer),
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
            Box::new(crate::answerer::DenyAllAnswerer),
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.to_lowercase().contains("unsatisfied") || err.contains("vlc"),
            "expected 'unsatisfied' or 'vlc' in error, got: {err}"
        );
    }
}
