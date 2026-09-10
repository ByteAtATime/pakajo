use std::process::Command;

use crate::cli::privs::{is_root, stdin_is_tty};

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
