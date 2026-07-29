use std::io::BufReader;
use std::process::{Child, Command, Stdio};

use anyhow::Context as _;

use super::privs::is_root;
use crate::events;

fn run_escalated_child(mut child: Child, json: bool) -> anyhow::Result<i32> {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink = super::sink_for(json);
    events::read_event_stream(BufReader::new(stdout), &mut *sink);
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

pub(super) fn escalate_result(
    targets: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<i32> {
    let child = crate::build::spawn_install_child(targets, as_deps, approvals_b64)?;
    run_escalated_child(child, json)
}

pub(super) fn escalate(
    targets: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> ! {
    let code = match escalate_result(targets, as_deps, json, approvals_b64) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(code);
}

pub(super) fn escalate_remove(targets: &[String], json: bool) -> ! {
    let child = match crate::remove::spawn_remove_child(targets) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    match run_escalated_child(child, json) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

pub(super) fn escalate_upgrade(no_refresh: bool, ignores: &[String], json: bool) -> i32 {
    let child = match spawn_upgrade_child(no_refresh, ignores) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    match run_escalated_child(child, json) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

fn spawn_upgrade_child(no_refresh: bool, ignores: &[String]) -> anyhow::Result<Child> {
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = escalation_command(&exe.to_string_lossy());
    cmd.arg("upgrade").arg("--json").arg("--repo-only");
    if no_refresh {
        cmd.arg("--no-refresh");
    }
    for name in ignores {
        cmd.arg("--ignore").arg(name);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn().context("failed to spawn upgrade child")
}

pub(crate) fn escalation_command(exe: &str) -> Command {
    if is_root() {
        Command::new(exe)
    } else {
        let mut command = Command::new("pkexec");
        command.arg(exe);
        command
    }
}
