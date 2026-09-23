use std::io::{self, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

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
    AnswerChannel(AnswerWriter),
    Done(ChildOutcome),
}

#[derive(Clone, Debug)]
pub struct AnswerWriter {
    writer: Arc<Mutex<ChildStdin>>,
}

impl AnswerWriter {
    pub fn from_stdin(stdin: ChildStdin) -> Self {
        Self {
            writer: Arc::new(Mutex::new(stdin)),
        }
    }

    pub fn answer(&self, yes: bool) {
        let line = if yes { "yes\n" } else { "no\n" };
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = writer.write_all(line.as_bytes());
        let _ = writer.flush();
    }
}

#[derive(Default)]
struct PromptDecliner {
    writer: Option<AnswerWriter>,
}

impl PromptDecliner {
    fn channel(&mut self, writer: AnswerWriter) {
        self.writer = Some(writer);
    }

    fn on_event(&self, event: &InstallEvent) {
        if matches!(event, InstallEvent::RuntimePrompt { .. })
            && let Some(writer) = self.writer.as_ref()
        {
            writer.answer(false);
        }
    }
}

pub type DispatchStream = futures::channel::mpsc::Receiver<StreamItem>;

pub fn drain_declining(
    mut stream: DispatchStream,
    sink: &mut (impl InstallSink + ?Sized),
) -> ChildOutcome {
    use futures::StreamExt as _;
    let mut answers = PromptDecliner::default();
    while let Some(item) = futures::executor::block_on(stream.next()) {
        match item {
            StreamItem::Event(event) => {
                answers.on_event(&event);
                sink.event(event);
            }
            StreamItem::AnswerChannel(writer) => answers.channel(writer),
            StreamItem::Done(outcome) => return outcome,
        }
    }
    ChildOutcome::Failed("stream ended".to_string())
}

pub fn send_done(tx: &mut futures::channel::mpsc::Sender<StreamItem>, outcome: ChildOutcome) {
    send_item(tx, StreamItem::Done(outcome));
}

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
) -> Option<(Child, &'static str, Option<AnswerWriter>)> {
    let (mut cmd, escalator) = escalation(exe, tty);
    cmd.arg(MARKER);
    for arg in argv {
        cmd.arg(arg);
    }
    if tty {
        cmd.stdin(Stdio::inherit());
    } else {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
    match cmd.spawn() {
        Ok(mut child) => {
            let writer = child
                .stdin
                .take()
                .filter(|_| !tty)
                .map(|stdin| AnswerWriter {
                    writer: Arc::new(Mutex::new(stdin)),
                });
            Some((child, escalator, writer))
        }
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
        PrivilegedOperation::Remove { approvals, .. } => approvals
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
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
    let Some((child, escalator, writer)) =
        spawn_privileged_child(&argv, &exe.to_string_lossy(), tty, tx)
    else {
        return;
    };
    if let Some(writer) = writer {
        send_item(tx, StreamItem::AnswerChannel(writer));
    }
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
            let result = crate::dispatch::aur::install_aur(
                crate::dispatch::aur::BuildParams {
                    targets: &self.targets,
                    files: &self.files,
                    no_check: self.no_check,
                    as_deps: self.as_deps,
                    reinstall: self.reinstall,
                    approvals: approvals.as_deref(),
                    tty,
                    interactive: self.interactive,
                },
                &mut sink,
                decider.as_ref(),
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
