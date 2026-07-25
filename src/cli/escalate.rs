use std::io::BufReader;
use std::process::{Child, Command, Stdio};

use anyhow::Context as _;

use super::privs::is_root;
use super::sinks::{ConsoleSink, JsonSink};
use crate::events::{self, InstallSink};

pub(super) fn escalate_remove(targets: &[String], json: bool) -> ! {
    let mut child = match crate::remove::spawn_remove_child(targets) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink: Box<dyn InstallSink> = if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    events::read_event_stream(BufReader::new(stdout), &mut *sink);
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(status.code().unwrap_or(1));
}

pub(super) fn escalate_upgrade(no_refresh: bool, ignores: &[String], json: bool) -> i32 {
    let mut child = match spawn_upgrade_child(no_refresh, ignores) {
        Ok(child) => child,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink: Box<dyn InstallSink> = if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    events::read_event_stream(BufReader::new(stdout), &mut *sink);
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    status.code().unwrap_or(1)
}

fn spawn_upgrade_child(
    no_refresh: bool,
    ignores: &[String],
) -> anyhow::Result<Child> {
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

pub(super) fn escalate(targets: &[String], as_deps: bool, json: bool) -> ! {
    let code = match escalate_result(targets, as_deps, json) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(code);
}

pub(super) fn escalate_result(targets: &[String], as_deps: bool, json: bool) -> anyhow::Result<i32> {
    let mut child = crate::build::spawn_install_child(targets, as_deps, None)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink: Box<dyn InstallSink> = if json {
        Box::new(JsonSink::new())
    } else {
        Box::new(ConsoleSink::new())
    };
    events::read_event_stream(BufReader::new(stdout), &mut *sink);
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
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
