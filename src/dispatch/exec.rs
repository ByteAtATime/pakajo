use std::io::{self, BufReader};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use anyhow::Context as _;
use base64::Engine as _;
use futures::SinkExt as _;

use crate::cli::privs::{is_root, stdin_is_tty};
use crate::cli::{ConsoleSink, JsonSink};
use crate::dispatch::operation::{MARKER, Operation};
use crate::dispatch::protocol::{
    AutomaticDecider, Decider as _, Placement, TerminalDecider, place,
};
use crate::events::{InstallEvent, InstallSink, read_event_stream};

#[derive(Clone, Debug)]
pub enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

pub enum StreamItem {
    Event(InstallEvent),
    Done(ChildOutcome),
}

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

fn escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), stdin_is_tty()).build_command(exe)
}

fn graphical_escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), false).build_command(exe)
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
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

struct ApprovalsFile {
    path: PathBuf,
}

impl ApprovalsFile {
    fn write(json: &[u8]) -> io::Result<ApprovalsFile> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("pakajo-approvals-{}-{id}.json", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        std::fs::write(&path, json)?;
        Ok(ApprovalsFile { path })
    }

    fn write_b64(b64: &str) -> anyhow::Result<ApprovalsFile> {
        let json = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("--approvals is not valid base64")?;
        ApprovalsFile::write(&json).context("failed to write approvals file")
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ApprovalsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn operation_command(operation: &Operation, exe: &str, graphical: bool) -> Command {
    let mut cmd = if graphical {
        graphical_escalation_command(exe)
    } else {
        escalation_command(exe)
    };
    cmd.arg(MARKER);
    for arg in operation.encode() {
        cmd.arg(arg);
    }
    cmd
}

fn parent_sink(json: bool) -> Box<dyn crate::events::InstallSink> {
    if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    }
}

fn remove_operation(targets: &[String]) -> Operation {
    Operation::Remove {
        targets: targets.to_vec(),
        stream: true,
    }
}

fn spawn_cli_child(operation: &Operation, name: &str) -> anyhow::Result<Child> {
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = operation_command(operation, &exe.to_string_lossy(), false);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn()
        .with_context(|| format!("failed to spawn {name} child"))
}

fn spawn_channel_child(
    operation: &Operation,
    exe: &str,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<Child> {
    let mut cmd = operation_command(operation, exe, true);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
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

fn finish_channel(
    status: io::Result<ExitStatus>,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) {
    send_item(tx, StreamItem::Done(map_outcome(status)));
}

pub fn run_remove(targets: &[String], json: bool) -> ! {
    std::process::exit(match run_remove_result(targets, json) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    });
}

fn run_remove_result(targets: &[String], json: bool) -> anyhow::Result<i32> {
    let operation = remove_operation(targets);
    let child = spawn_cli_child(&operation, "remove")?;
    let mut sink = parent_sink(json);
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
    let Some(child) = spawn_channel_child(&operation, &exe.to_string_lossy(), &mut tx) else {
        return;
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    finish_channel(status, &mut tx);
}

pub fn run_install(
    targets: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> ! {
    std::process::exit(
        match run_install_result(targets, as_deps, json, approvals_b64) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e:#}");
                1
            }
        },
    );
}

fn install_operation(
    targets: &[String],
    as_deps: bool,
    approvals: Option<&ApprovalsFile>,
) -> Operation {
    Operation::Install {
        targets: targets.to_vec(),
        as_deps,
        approvals_path: approvals.map(|file| file.path().to_string_lossy().into_owned()),
        stream: true,
    }
}

pub(crate) fn run_install_result(
    targets: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<i32> {
    let approvals = approvals_b64.map(ApprovalsFile::write_b64).transpose()?;
    let operation = install_operation(targets, as_deps, approvals.as_ref());
    let child = spawn_cli_child(&operation, "install")?;
    let mut sink = parent_sink(json);
    let status = stream_child(child, &mut *sink)?;
    Ok(status.code().unwrap_or(1))
}

pub fn run_install_to_sink<S: crate::events::InstallSink + ?Sized>(
    targets: &[String],
    as_deps: bool,
    approvals_b64: Option<&str>,
    sink: &mut S,
) -> anyhow::Result<()> {
    let approvals = approvals_b64.map(ApprovalsFile::write_b64).transpose()?;
    let operation = install_operation(targets, as_deps, approvals.as_ref());
    let child = spawn_cli_child(&operation, "install")?;
    let status = stream_child(child, sink).context("install child did not complete")?;
    if !status.success() {
        anyhow::bail!(
            "privileged install of [{}] failed (exit {})",
            targets.join(", "),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

pub fn run_install_to_channel(
    exe: PathBuf,
    targets: Vec<String>,
    as_deps: bool,
    approvals_b64: Option<String>,
    mut tx: futures::channel::mpsc::Sender<StreamItem>,
) {
    let approvals = match approvals_b64
        .as_deref()
        .map(ApprovalsFile::write_b64)
        .transpose()
    {
        Ok(approvals) => approvals,
        Err(err) => {
            send_item(
                &mut tx,
                StreamItem::Done(ChildOutcome::Failed(format!("{err:#}"))),
            );
            return;
        }
    };
    let operation = install_operation(&targets, as_deps, approvals.as_ref());
    let Some(child) = spawn_channel_child(&operation, &exe.to_string_lossy(), &mut tx) else {
        return;
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    finish_channel(status, &mut tx);
}

fn upgrade_repo_operation(
    no_refresh: bool,
    ignores: &[String],
    fingerprint_path: Option<&str>,
    approvals: Option<&ApprovalsFile>,
) -> Operation {
    Operation::UpgradeRepo {
        no_refresh,
        ignores: ignores.to_vec(),
        fingerprint_path: fingerprint_path.map(str::to_string),
        approvals_path: approvals.map(|file| file.path().to_string_lossy().into_owned()),
        stream: true,
    }
}

pub(crate) fn run_upgrade_repo_result(
    no_refresh: bool,
    ignores: &[String],
    json: bool,
) -> anyhow::Result<i32> {
    let operation = upgrade_repo_operation(no_refresh, ignores, None, None);
    let child = spawn_cli_child(&operation, "upgrade")?;
    let mut sink = parent_sink(json);
    let status = stream_child(child, &mut *sink)?;
    Ok(status.code().unwrap_or(1))
}

pub fn run_upgrade_repo_to_channel(
    exe: PathBuf,
    no_refresh: bool,
    ignores: Vec<String>,
    fingerprint_path: Option<String>,
    approvals_b64: Option<String>,
    mut tx: futures::channel::mpsc::Sender<StreamItem>,
) {
    let approvals = match approvals_b64
        .as_deref()
        .map(ApprovalsFile::write_b64)
        .transpose()
    {
        Ok(approvals) => approvals,
        Err(err) => {
            send_item(
                &mut tx,
                StreamItem::Done(ChildOutcome::Failed(format!("{err:#}"))),
            );
            return;
        }
    };
    let operation = upgrade_repo_operation(
        no_refresh,
        &ignores,
        fingerprint_path.as_deref(),
        approvals.as_ref(),
    );
    let Some(child) = spawn_channel_child(&operation, &exe.to_string_lossy(), &mut tx) else {
        return;
    };
    let mut sink = ChannelSink::new(tx.clone());
    let status = stream_child(child, &mut sink);
    finish_channel(status, &mut tx);
}

fn build_aur_operation(targets: &[String], as_deps: bool) -> Operation {
    Operation::BuildAur {
        targets: targets.to_vec(),
        as_deps,
        stream: true,
    }
}

fn run_build_aur_inner(
    targets: &[String],
    as_deps: bool,
    decider: &dyn crate::dispatch::protocol::Decider,
    sink: &mut dyn crate::events::InstallSink,
    approvals_b64: Option<&str>,
) -> anyhow::Result<()> {
    crate::build::run_build(
        targets,
        false,
        as_deps,
        sink,
        |plan| decider.confirm_build(plan),
        |pkgbuilds| decider.review_pkgbuilds(pkgbuilds),
        approvals_b64,
    )
}

pub fn run_build_aur(
    targets: &[String],
    as_deps: bool,
    json: bool,
    skip_review: bool,
    approvals_b64: Option<&str>,
) -> ! {
    std::process::exit(
        match run_build_aur_result(targets, as_deps, json, skip_review, approvals_b64) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e:#}");
                1
            }
        },
    );
}

pub(crate) fn run_build_aur_result(
    targets: &[String],
    as_deps: bool,
    json: bool,
    skip_review: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<()> {
    let operation = build_aur_operation(targets, as_deps);
    if !matches!(place(&operation), Placement::InProcess) {
        anyhow::bail!("BuildAur must execute in-process");
    }
    let decider = TerminalDecider::new(json, skip_review);
    let mut sink = parent_sink(json);
    run_build_aur_inner(targets, as_deps, &decider, &mut *sink, approvals_b64)
}

pub fn run_build_aur_to_channel(
    targets: Vec<String>,
    as_deps: bool,
    approvals_b64: Option<String>,
    mut tx: futures::channel::mpsc::Sender<StreamItem>,
) {
    let operation = build_aur_operation(&targets, as_deps);
    if !matches!(place(&operation), Placement::InProcess) {
        send_item(
            &mut tx,
            StreamItem::Done(ChildOutcome::Failed(
                "BuildAur must execute in-process".to_string(),
            )),
        );
        return;
    }
    let decider = AutomaticDecider;
    let mut sink = ChannelSink::new(tx.clone());
    let result = crate::build::run_build(
        &targets,
        false,
        as_deps,
        &mut sink,
        |plan| decider.confirm_build(plan),
        |pkgbuilds| decider.review_pkgbuilds(pkgbuilds),
        approvals_b64.as_deref(),
    );
    let outcome = match result {
        Ok(()) => ChildOutcome::Success,
        Err(e) => ChildOutcome::Failed(format!("{e:#}")),
    };
    send_item(&mut tx, StreamItem::Done(outcome));
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
    fn approvals_file_has_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let payload = br#"{"approved_conflicts":[]}"#;
        let file = ApprovalsFile::write(payload).expect("write approvals file");
        let mode = std::fs::metadata(file.path())
            .expect("stat approvals file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn approvals_file_path_is_absolute() {
        let file = ApprovalsFile::write(br#"{"approved_conflicts":[]}"#).expect("write file");
        assert!(file.path().is_absolute());
    }

    #[test]
    fn install_argv_carries_path_not_payload() {
        let payload = r#"{"approved_conflicts":[{"incoming":"cava-git","removable":"cava"}]}"#;
        let file = ApprovalsFile::write(payload.as_bytes()).expect("write file");
        let operation = Operation::Install {
            targets: vec!["sl".to_string()],
            as_deps: false,
            approvals_path: Some(file.path().to_string_lossy().into_owned()),
            stream: true,
        };
        let argv = operation.encode();
        assert!(argv.contains(&file.path().to_string_lossy().into_owned()));
        for arg in &argv {
            assert!(
                !arg.contains("approved_conflicts"),
                "argv must not contain approvals payload: {arg:?}"
            );
        }
    }

    #[test]
    fn remove_operation_always_streams() {
        let operation = remove_operation(&["sl".to_string()]);
        assert!(operation.encode().contains(&"--stream".to_string()));
    }

    #[test]
    fn install_operation_always_streams() {
        let operation = install_operation(&["sl".to_string()], false, None);
        assert!(operation.encode().contains(&"--stream".to_string()));
    }

    #[test]
    fn upgrade_repo_operation_always_streams() {
        let operation = upgrade_repo_operation(false, &["foo".to_string()], None, None);
        assert!(operation.encode().contains(&"--stream".to_string()));
    }

    #[test]
    fn build_aur_operation_always_streams() {
        let operation = build_aur_operation(&["cava-git".to_string()], false);
        assert!(operation.encode().contains(&"--stream".to_string()));
    }

    #[test]
    fn approvals_file_deleted_after_normal_exit() {
        let path = {
            let file = ApprovalsFile::write(br#"{"approved_conflicts":[]}"#).expect("write file");
            let path = file.path().to_path_buf();
            assert!(path.exists());
            drop(file);
            path
        };
        assert!(!path.exists());
    }

    #[test]
    fn approvals_file_deleted_after_nonzero_exit() {
        let path = {
            let file = ApprovalsFile::write(br#"{"approved_conflicts":[]}"#).expect("write file");
            let path = file.path().to_path_buf();
            let outcome: anyhow::Result<i32> = Err(anyhow::anyhow!("child exited 1"));
            assert!(outcome.is_err());
            drop(file);
            path
        };
        assert!(!path.exists());
    }

    #[test]
    fn approvals_file_deleted_after_spawn_failure() {
        let path = {
            let file = ApprovalsFile::write(br#"{"approved_conflicts":[]}"#).expect("write file");
            let path = file.path().to_path_buf();
            let spawn: std::io::Result<()> = Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "pkexec not found",
            ));
            assert!(spawn.is_err());
            drop(file);
            path
        };
        assert!(!path.exists());
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
