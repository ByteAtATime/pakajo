use std::io::BufReader;
use std::process::{Child, Command, Stdio};

use super::privs::{is_root, stdin_is_tty};
use crate::events;
use crate::subprocess::{ChildJob, spawn_escalated};

fn run_escalated_child(mut child: Child, json: bool) -> anyhow::Result<i32> {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut sink = super::sink_for(json);
    events::read_event_stream(BufReader::new(stdout), &mut *sink);
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

pub fn escalate_result(
    targets: &[String],
    as_deps: bool,
    json: bool,
    approvals_b64: Option<&str>,
) -> anyhow::Result<i32> {
    let child = spawn_escalated(
        &ChildJob::Install {
            targets: targets.to_vec(),
            as_deps,
            approvals_b64: approvals_b64.map(String::from),
        },
        Stdio::inherit(),
    )?;
    run_escalated_child(child, json)
}

pub fn escalate(targets: &[String], as_deps: bool, json: bool, approvals_b64: Option<&str>) -> ! {
    let code = match escalate_result(targets, as_deps, json, approvals_b64) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    std::process::exit(code);
}

pub fn escalate_remove(targets: &[String], json: bool) -> ! {
    let child = match spawn_escalated(
        &ChildJob::Remove {
            targets: targets.to_vec(),
        },
        Stdio::inherit(),
    ) {
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

pub fn escalate_upgrade(no_refresh: bool, ignores: &[String], json: bool) -> i32 {
    let child = match spawn_escalated(
        &ChildJob::Upgrade {
            no_refresh,
            ignores: ignores.to_vec(),
            fingerprint_file: None,
            approvals_b64: None,
        },
        Stdio::inherit(),
    ) {
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

pub fn escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), stdin_is_tty()).build_command(exe)
}

pub fn graphical_escalation_command(exe: &str) -> Command {
    select_privilege_escalator(is_root(), false).build_command(exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

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
}
