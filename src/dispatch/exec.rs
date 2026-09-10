use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use anyhow::Context as _;
use base64::Engine as _;

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

pub struct ApprovalsFile {
    path: PathBuf,
}

impl ApprovalsFile {
    pub fn write(json: &[u8]) -> io::Result<ApprovalsFile> {
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

    pub fn write_b64(b64: &str) -> anyhow::Result<ApprovalsFile> {
        let json = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("--approvals is not valid base64")?;
        ApprovalsFile::write(&json).context("failed to write approvals file")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ApprovalsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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

pub fn run_install(
    targets: &[String],
    as_deps: bool,
    stream: bool,
    approvals_b64: Option<&str>,
) -> ! {
    std::process::exit(
        match run_install_result(targets, as_deps, stream, approvals_b64) {
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
    stream: bool,
    approvals: Option<&ApprovalsFile>,
) -> Operation {
    Operation::Install {
        targets: targets.to_vec(),
        as_deps,
        approvals_path: approvals.map(|file| file.path().to_string_lossy().into_owned()),
        stream,
    }
}

fn install_command(operation: &Operation, exe: &str, graphical: bool) -> Command {
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

pub(crate) fn run_install_result(
    targets: &[String],
    as_deps: bool,
    stream: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<i32> {
    let approvals = approvals_b64.map(ApprovalsFile::write_b64).transpose()?;
    let operation = install_operation(targets, as_deps, stream, approvals.as_ref());
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = install_command(&operation, &exe.to_string_lossy(), false);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let child = cmd.spawn().context("failed to spawn install child")?;
    let mut sink: Box<dyn crate::events::InstallSink> = if stream {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
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
    let operation = install_operation(targets, as_deps, true, approvals.as_ref());
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = install_command(&operation, &exe.to_string_lossy(), false);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let child = cmd.spawn().context("failed to spawn install child")?;
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
    let operation = install_operation(&targets, as_deps, true, approvals.as_ref());
    let mut cmd = install_command(&operation, &exe.to_string_lossy(), true);
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
