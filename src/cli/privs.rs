pub fn is_root() -> bool {
    (unsafe { libc::geteuid() }) == 0
}

pub fn stdin_is_tty() -> bool {
    (unsafe { libc::isatty(0) }) == 1
}
