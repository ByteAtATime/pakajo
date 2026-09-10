use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::Context as _;

use crate::cli::privs::{is_root, stdin_is_tty};
use crate::cli::{ConsoleSink, JsonSink};
use crate::dispatch::operation::{MARKER, Operation};
use crate::subprocess::{ChannelSink, ChildOutcome, StreamItem, send_item, stream_child};

pub trait PrivilegeEscalator {
    fn build_command(&self, exe: &str) -> Command;
}

struct Direct;

impl PrivilegeEscalator for Direct {
    fn build_command(&self, exe: &str) -> Command {
        Command::new(exe)
    }
}

struct Pkexec;

impl PrivilegeEscalator for Pkexec {
    fn build_command(&self, exe: &str) -> Command {
        let mut command = Command::new("pkexec");
        command.arg(exe);
        command
    }
}

struct Sudo;

impl PrivilegeEscalator for Sudo {
    fn build_command(&self, exe: &str) -> Command {
        let mut command = Command::new("sudo");
        command.arg(exe);
        command
    }
}

pub(crate) fn select_privilege_escalator(root: bool, tty: bool) -> Box<dyn PrivilegeEscalator> {
    if root {
        Box::new(Direct)
    } else if tty {
        Box::new(Sudo)
    } else {
        Box::new(Pkexec)
    }
}

pub fn escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), stdin_is_tty()).build_command(exe)
}

pub fn graphical_escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), false).build_command(exe)
}

pub fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

pub fn run_remove(targets: &[String], stream: bool) -> ! {
    std::process::exit(match run_remove_result(targets, stream) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    });
}

fn run_remove_result(targets: &[String], stream: bool) -> anyhow::Result<i32> {
    let operation = Operation::Remove {
        targets: targets.to_vec(),
        stream,
    };
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = escalation_command(&exe.to_string_lossy());
    cmd.arg(MARKER);
    for arg in operation.encode() {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let child = cmd.spawn().context("failed to spawn remove child")?;
    let mut sink: Box<dyn crate::events::InstallSink> = if stream {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    let status = stream_child(child, &mut *sink)?;
    Ok(status.code().unwrap_or(1))
}

pub fn run_remove_to_channel(
    exe: PathBuf,
    targets: Vec<String>,
    mut tx: futures::channel::mpsc::Sender<StreamItem>,
) {
    let operation = Operation::Remove {
        targets,
        stream: true,
    };
    let mut cmd = graphical_escalation_command(&exe.to_string_lossy());
    cmd.arg(MARKER);
    for arg in operation.encode() {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            send_item(&mut tx, StreamItem::Done(ChildOutcome::NotFound));
            return;
        }
        Err(err) => {
            send_item(
                &mut tx,
                StreamItem::Done(ChildOutcome::Failed(err.to_string())),
            );
            return;
        }
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    send_item(&mut tx, StreamItem::Done(map_outcome(status)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn root_uses_direct() {
        let cmd = select_privilege_escalator(true, false).build_command("x");
        assert_eq!(cmd.get_program(), OsStr::new("x"));
        assert_eq!(cmd.get_args().count(), 0);
    }

    #[test]
    fn root_dominates_tty() {
        let cmd = select_privilege_escalator(true, true).build_command("x");
        assert_eq!(cmd.get_program(), OsStr::new("x"));
    }

    #[test]
    fn non_root_on_tty_uses_sudo() {
        let cmd = select_privilege_escalator(false, true).build_command("x");
        assert_eq!(cmd.get_program(), OsStr::new("sudo"));
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(args, [OsStr::new("x")]);
    }

    #[test]
    fn non_root_off_tty_uses_pkexec() {
        let cmd = select_privilege_escalator(false, false).build_command("x");
        assert_eq!(cmd.get_program(), OsStr::new("pkexec"));
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(args, [OsStr::new("x")]);
    }

    #[test]
    fn outcome_maps_exit_vocabulary() {
        let cases: Vec<(io::Result<ExitStatus>, ChildOutcome)> = vec![
            (Ok(ExitStatusExt::from_raw(0)), ChildOutcome::Success),
            (
                Ok(ExitStatusExt::from_raw(126 << 8)),
                ChildOutcome::Dismissed,
            ),
            (
                Ok(ExitStatusExt::from_raw(127 << 8)),
                ChildOutcome::NotFound,
            ),
            (
                Ok(ExitStatusExt::from_raw(1 << 8)),
                ChildOutcome::Failed("install failed (exit 1)".to_string()),
            ),
            (
                Ok(ExitStatusExt::from_raw(9)),
                ChildOutcome::Failed("install killed by signal".to_string()),
            ),
            (
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream broke")),
                ChildOutcome::Failed("stream broke".to_string()),
            ),
        ];
        for (status, expected) in cases {
            let actual = map_outcome(status);
            assert_eq!(
                format!("{actual:?}"),
                format!("{expected:?}"),
                "map_outcome"
            );
        }
    }
}
