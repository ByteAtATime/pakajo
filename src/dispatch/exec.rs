use std::io::{self, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};

use anyhow::Context as _;
use base64::Engine as _;
use futures::SinkExt as _;

use crate::cli::privs::is_root;
use crate::dispatch::approvals::ApprovalsFile;
use crate::dispatch::operation::{BuildOperation, MARKER, PrivilegedOperation};
use crate::dispatch::protocol::Decider;
use crate::events::{InstallEvent, InstallSink, read_event_stream};

#[derive(Clone, Debug)]
pub enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

impl ChildOutcome {
    pub fn reason(&self) -> &str {
        match self {
            ChildOutcome::Success => "succeeded",
            ChildOutcome::Dismissed => "privilege prompt dismissed",
            ChildOutcome::NotFound => "pkexec not found",
            ChildOutcome::Failed(message) => message.as_str(),
        }
    }
}

pub enum StreamItem {
    Event(InstallEvent),
    Done(ChildOutcome),
}

pub type DispatchStream = futures::channel::mpsc::Receiver<StreamItem>;

fn send_item(tx: &mut futures::channel::mpsc::Sender<StreamItem>, item: StreamItem) {
    futures::executor::block_on(tx.send(item)).ok();
}

pub struct ChannelSink {
    tx: futures::channel::mpsc::Sender<StreamItem>,
}

impl ChannelSink {
    pub fn new(tx: futures::channel::mpsc::Sender<StreamItem>) -> Self {
        Self { tx }
    }
}

impl InstallSink for ChannelSink {
    fn event(&mut self, event: InstallEvent) {
        send_item(&mut self.tx, StreamItem::Event(event));
    }
}

fn stream_child<S: InstallSink + ?Sized>(
    mut child: Child,
    sink: &mut S,
) -> std::io::Result<ExitStatus> {
    let stdout = child.stdout.take().expect("piped stdout");
    read_event_stream(BufReader::new(stdout), sink);
    child.wait()
}

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

fn select_privilege_escalator(root: bool, tty: bool) -> Box<dyn PrivilegeEscalator> {
    if root {
        Box::new(Direct)
    } else if tty {
        Box::new(Sudo)
    } else {
        Box::new(Pkexec)
    }
}

fn escalation(exe: &str, tty: bool) -> Command {
    select_privilege_escalator(is_root(), tty).build_command(exe)
}

fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("operation failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

fn write_b64(b64: &str) -> anyhow::Result<ApprovalsFile> {
    let json = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("--approvals is not valid base64")?;
    ApprovalsFile::write(&json).context("failed to write approvals file")
}

fn spawn_privileged_child(
    argv: &[String],
    exe: &str,
    tty: bool,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<Child> {
    let mut cmd = escalation(exe, tty);
    cmd.arg(MARKER);
    for arg in argv {
        cmd.arg(arg);
    }
    if tty {
        cmd.stdin(Stdio::inherit());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            send_item(tx, StreamItem::Done(ChildOutcome::NotFound));
            None
        }
        Err(err) => {
            send_item(tx, StreamItem::Done(ChildOutcome::Failed(err.to_string())));
            None
        }
    }
}

fn run_privileged(
    operation: PrivilegedOperation,
    tty: bool,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) {
    let approvals_b64 = match &operation {
        PrivilegedOperation::Remove { .. } => None,
        PrivilegedOperation::Install { approvals, .. } => approvals.as_deref(),
        PrivilegedOperation::UpgradeRepo { approvals, .. } => approvals.as_deref(),
    };
    let sealed = match approvals_b64.map(write_b64).transpose() {
        Ok(sealed) => sealed,
        Err(err) => {
            send_item(
                tx,
                StreamItem::Done(ChildOutcome::Failed(format!("{err:#}"))),
            );
            return;
        }
    };
    let fingerprint_path = match &operation {
        PrivilegedOperation::UpgradeRepo { fingerprint, .. } => fingerprint
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        _ => None,
    };
    let approvals_path = sealed
        .as_ref()
        .map(|file| file.path().to_string_lossy().into_owned());
    let argv = operation.wire_args(approvals_path.as_deref(), fingerprint_path.as_deref());
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            send_item(
                tx,
                StreamItem::Done(ChildOutcome::Failed(format!("{err:#}"))),
            );
            return;
        }
    };
    let Some(child) = spawn_privileged_child(&argv, &exe.to_string_lossy(), tty, tx) else {
        return;
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    send_item(tx, StreamItem::Done(map_outcome(status)));
}

impl PrivilegedOperation {
    pub fn dispatch(self, tty: bool) -> DispatchStream {
        let (tx, rx) = futures::channel::mpsc::channel(256);
        std::thread::spawn(move || {
            let mut tx = tx;
            run_privileged(self, tty, &mut tx);
        });
        rx
    }
}

impl BuildOperation {
    pub fn dispatch(
        self,
        decider: Box<dyn Decider + Send>,
        approvals: Option<String>,
        tty: bool,
    ) -> DispatchStream {
        let (tx, rx) = futures::channel::mpsc::channel(256);
        std::thread::spawn(move || {
            let mut tx = tx;
            let mut sink = ChannelSink::new(tx.clone());
            let result = crate::build::run_build(
                &self.targets,
                false,
                self.as_deps,
                &mut sink,
                decider.as_ref(),
                approvals.as_deref(),
                tty,
            );
            let outcome = match result {
                Ok(()) => ChildOutcome::Success,
                Err(e) => ChildOutcome::Failed(format!("{e:#}")),
            };
            send_item(&mut tx, StreamItem::Done(outcome));
        });
        rx
    }
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
                ChildOutcome::Failed("operation failed (exit 1)".to_string()),
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
