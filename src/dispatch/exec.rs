use std::io::{self, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};

use futures::SinkExt as _;

use crate::cli::privs::is_root;
use crate::dispatch::operation::{BuildOperation, MARKER, PrivilegedOperation};
use crate::dispatch::protocol::Decider;
use crate::events::{InstallEvent, InstallSink, read_event_stream};

#[derive(Clone, Debug)]
pub enum ChildOutcome {
    Success,
    Dismissed,
    NotFound(String),
    Failed(String),
}

impl ChildOutcome {
    pub fn reason(&self) -> &str {
        match self {
            ChildOutcome::Success => "succeeded",
            ChildOutcome::Dismissed => "privilege prompt dismissed",
            ChildOutcome::NotFound(message) => message.as_str(),
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
    if let Err(error) = futures::executor::block_on(tx.send(item)) {
        eprintln!("warning: dispatch stream closed: {error}");
    }
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
    fn name(&self) -> &'static str;
}

struct Direct;

impl PrivilegeEscalator for Direct {
    fn build_command(&self, exe: &str) -> Command {
        Command::new(exe)
    }

    fn name(&self) -> &'static str {
        "direct"
    }
}

struct Pkexec;

impl PrivilegeEscalator for Pkexec {
    fn build_command(&self, exe: &str) -> Command {
        let mut command = Command::new("pkexec");
        command.arg(exe);
        command
    }

    fn name(&self) -> &'static str {
        "pkexec"
    }
}

struct Sudo;

impl PrivilegeEscalator for Sudo {
    fn build_command(&self, exe: &str) -> Command {
        let mut command = Command::new("sudo");
        command.arg(exe);
        command
    }

    fn name(&self) -> &'static str {
        "sudo"
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

fn escalation(exe: &str, tty: bool) -> (Command, &'static str) {
    let escalator = select_privilege_escalator(is_root(), tty);
    let name = escalator.name();
    (escalator.build_command(exe), name)
}

fn map_outcome(status: io::Result<ExitStatus>, escalator: &str) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound(format!("{escalator} not found")),
        Some(exit) => ChildOutcome::Failed(format!("operation failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

fn spawn_privileged_child(
    argv: &[String],
    exe: &str,
    tty: bool,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<(Child, &'static str)> {
    let (mut cmd, escalator) = escalation(exe, tty);
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
        Ok(child) => Some((child, escalator)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            send_item(
                tx,
                StreamItem::Done(ChildOutcome::NotFound(format!("{escalator} not found"))),
            );
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
    let fingerprint_path = match &operation {
        PrivilegedOperation::UpgradeRepo { fingerprint, .. } => fingerprint
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        _ => None,
    };
    let approvals_path = match &operation {
        PrivilegedOperation::Remove { .. } => None,
        PrivilegedOperation::Install { approvals, .. } => approvals
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        PrivilegedOperation::UpgradeRepo { approvals, .. } => approvals
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
    };
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
    let Some((child, escalator)) = spawn_privileged_child(&argv, &exe.to_string_lossy(), tty, tx)
    else {
        return;
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    send_item(tx, StreamItem::Done(map_outcome(status, escalator)));
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
                ChildOutcome::NotFound("sudo not found".to_string()),
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
            let actual = map_outcome(status, "sudo");
            assert_eq!(
                format!("{actual:?}"),
                format!("{expected:?}"),
                "map_outcome"
            );
        }
    }

    #[test]
    fn missing_escalator_names_selected_backend() {
        let actual = map_outcome(Ok(ExitStatusExt::from_raw(127 << 8)), "pkexec");
        assert_eq!(actual.reason(), "pkexec not found");
    }
}
