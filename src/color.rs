use std::io::IsTerminal;

pub const COLON: &str = "\x1b[1;34m";
pub const BOLD: &str = "\x1b[0;1m";
pub const CYAN: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";

pub fn stdout_color() -> bool {
    std::io::stdout().is_terminal()
}

pub fn stderr_color() -> bool {
    std::io::stderr().is_terminal()
}

pub fn paint(enabled: bool, code: &str, s: &str) -> String {
    if enabled {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

pub fn colon(enabled: bool, msg: &str) -> String {
    if enabled {
        format!("{COLON}::{BOLD} {msg}{RESET}")
    } else {
        format!(":: {msg}")
    }
}
