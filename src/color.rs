use std::io::IsTerminal;
use std::sync::LazyLock;

static ANSI_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[0-9;?]*[A-Za-z]|\(B)")
        .expect("valid ansi strip regex")
});

pub fn ansi_strip(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

pub const COLON: &str = "\x1b[1;34m";
pub const BOLD: &str = "\x1b[0;1m";
pub const VERSION: &str = "\x1b[38;5;243m";
pub const CYAN: &str = "\x1b[36m";
pub const RED: &str = "\x1b[1;31m";
pub const YELLOW: &str = "\x1b[1;33m";
pub const WHITE: &str = "\x1b[0;37m";
pub const MAGENTA: &str = "\x1b[1;35m";
pub const GREEN: &str = "\x1b[1;32m";
pub const DIM: &str = "\x1b[2m\x1b[90m";
pub const GRAY: &str = "\x1b[90m";
pub const RESET: &str = "\x1b[0m";

fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

pub fn stdout_color() -> bool {
    std::io::stdout().is_terminal() && !no_color_requested()
}

pub fn stderr_color() -> bool {
    std::io::stderr().is_terminal() && !no_color_requested()
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

pub fn visible_width(s: &str) -> usize {
    let mut count = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            count += 1;
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some('(') | Some(')') => {
                let _ = chars.next();
            }
            _ => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::{ansi_strip, visible_width};

    #[test]
    fn ansi_strip_table() {
        let cases = [
            ("plain text", "plain text"),
            ("\x1b[1;32mok\x1b[0m", "ok"),
            ("\x1b]0;window title\x07done", "done"),
            ("\x1b]8;;https://example.com\x1b\\link", "link"),
        ];
        for (input, expected) in cases {
            assert_eq!(ansi_strip(input), expected, "input {input:?}");
        }
    }

    #[test]
    fn visible_width_bold_then_reset() {
        assert_eq!(visible_width("\x1b[1mhi\x1b[0m"), 2);
    }

    #[test]
    fn visible_width_256_color_version() {
        assert_eq!(visible_width("\x1b[38;5;243mv1.0-1\x1b[0m"), 6);
    }

    #[test]
    fn visible_width_colon_form() {
        assert_eq!(visible_width("\x1b[1;34m::\x1b[0;1m hi there\x1b[0m"), 11);
    }

    #[test]
    fn visible_width_plain() {
        assert_eq!(visible_width("plain"), 5);
    }

    #[test]
    fn visible_width_empty() {
        assert_eq!(visible_width(""), 0);
    }
}
