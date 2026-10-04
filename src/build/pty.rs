use std::collections::VecDeque;
use std::io::Read as _;
use std::os::unix::io::AsRawFd as _;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context as _;

pub(crate) struct PtySession {
    child: std::process::Child,
    master: std::fs::File,
    pending: Vec<u8>,
    child_gone: bool,
    ready: VecDeque<String>,
}

impl PtySession {
    pub(crate) fn spawn(
        cmd: &mut std::process::Command,
        separate_group: bool,
    ) -> anyhow::Result<Self> {
        use std::os::unix::process::CommandExt as _;
        if separate_group {
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let win_size = crate::utils::terminal_winsize();
        let pty = nix::pty::openpty(&win_size, None).context("failed to open pseudoterminal")?;
        let slave_stdout = pty.slave.try_clone().context("failed to clone pty slave")?;
        let slave_stderr = pty.slave.try_clone().context("failed to clone pty slave")?;
        cmd.stdout(Stdio::from(slave_stdout))
            .stderr(Stdio::from(slave_stderr));
        let child = cmd.spawn().context("failed to spawn makepkg")?;
        drop(pty.slave);
        let master = std::fs::File::from(pty.master);
        set_nonblocking(&master).context("failed to set pty master non-blocking")?;
        Ok(PtySession {
            child,
            master,
            pending: Vec::new(),
            child_gone: false,
            ready: VecDeque::new(),
        })
    }

    pub(crate) fn child_id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn next_line(&mut self) -> anyhow::Result<Option<String>> {
        if let Some(line) = self.ready.pop_front() {
            return Ok(Some(line));
        }
        let mut buf = [0u8; 4096];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut lines = Vec::new();
                    drain_lines(&mut self.pending, &buf[..n], &mut lines);
                    let mut iter = lines.into_iter();
                    if let Some(first) = iter.next() {
                        for line in iter {
                            self.ready.push_back(line);
                        }
                        return Ok(Some(first));
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if !self.child_gone {
                        match self.child.try_wait()? {
                            None => {
                                std::thread::sleep(Duration::from_millis(5));
                                continue;
                            }
                            Some(_) => self.child_gone = true,
                        }
                    }
                    let mut pfd = libc::pollfd {
                        fd: self.master.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    let ready = unsafe { libc::poll(&mut pfd, 1, 200) };
                    if ready <= 0 {
                        break;
                    }
                }
                Err(e) => return Err(e).context("failed to read makepkg output"),
            }
        }
        Ok(flush_pending(&mut self.pending))
    }

    pub(crate) fn wait(&mut self) -> anyhow::Result<std::process::ExitStatus> {
        self.child.wait().context("makepkg did not complete")
    }
}

fn set_nonblocking(file: &std::fs::File) -> anyhow::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error()).context("fcntl F_GETFL on pty master");
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error()).context("fcntl F_SETFL on pty master");
    }
    Ok(())
}

fn strip_trailing_cr(text: &mut String) {
    if text.ends_with('\r') {
        text.pop();
    }
}

fn drain_lines(pending: &mut Vec<u8>, chunk: &[u8], out: &mut Vec<String>) {
    pending.extend_from_slice(chunk);
    while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = pending.drain(..=pos).collect();
        let content = &line_bytes[..line_bytes.len() - 1];
        let mut text = String::from_utf8_lossy(content).into_owned();
        strip_trailing_cr(&mut text);
        out.push(text);
    }
}

fn flush_pending(pending: &mut Vec<u8>) -> Option<String> {
    if pending.is_empty() {
        return None;
    }
    let mut text = String::from_utf8_lossy(pending).into_owned();
    strip_trailing_cr(&mut text);
    pending.clear();
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    fn test_command() -> std::process::Command {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        cmd
    }

    fn pgid_of(pid: u32) -> i32 {
        unsafe { libc::getpgid(pid as i32) }
    }

    fn kill_group(pgid: i32) {
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }

    #[test]
    fn separate_group_spawns_own_process_group() {
        let mut cmd = test_command();
        let mut session = PtySession::spawn(&mut cmd, true).expect("spawn grouped sleep");
        let pid = session.child_id() as i32;
        assert_eq!(pgid_of(pid as u32), pid);
        kill_group(pid);
        let _ = session.wait();
    }

    #[test]
    fn shared_group_inherits_parent_process_group() {
        let mut cmd = test_command();
        let mut session = PtySession::spawn(&mut cmd, false).expect("spawn shared sleep");
        let own = pgid_of(unsafe { libc::getpid() } as u32);
        assert_eq!(pgid_of(session.child_id()), own);
        unsafe {
            libc::kill(session.child_id() as i32, libc::SIGKILL);
        }
        let _ = session.wait();
    }

    #[test]
    fn group_cancel_interrupts_session_child() {
        use crate::dispatch::cancel::{CancelHandle, CancelTarget};
        let mut cmd = test_command();
        let mut session = PtySession::spawn(&mut cmd, true).expect("spawn cancellable sleep");
        let handle = CancelHandle::default();
        handle.register(CancelTarget::Group(session.child_id() as i32));
        handle.cancel();
        let status = session.wait().expect("wait cancelled sleep");
        assert_eq!(status.signal(), Some(libc::SIGINT));
    }

    #[test]
    fn drain_lines_multiple_lines_one_chunk() {
        let mut pending = Vec::new();
        let mut out = Vec::new();
        drain_lines(&mut pending, b"alpha\nbeta\n", &mut out);
        assert_eq!(out, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(pending.is_empty());
    }

    #[test]
    fn drain_lines_partial_retained_across_calls() {
        let mut pending = Vec::new();
        let mut out = Vec::new();
        drain_lines(&mut pending, b"alpha\nbeta", &mut out);
        assert_eq!(out, vec!["alpha".to_string()]);
        assert_eq!(pending, b"beta");
        drain_lines(&mut pending, b"gamma\n", &mut out);
        assert_eq!(out, vec!["alpha".to_string(), "betagamma".to_string()]);
        assert!(pending.is_empty());
    }

    #[test]
    fn drain_lines_strips_crlf() {
        let mut pending = Vec::new();
        let mut out = Vec::new();
        drain_lines(&mut pending, b"alpha\r\nbeta\r\n", &mut out);
        assert_eq!(out, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(pending.is_empty());
    }

    #[test]
    fn drain_lines_empty_chunk_is_noop() {
        let mut pending: Vec<u8> = Vec::new();
        let mut out = Vec::new();
        drain_lines(&mut pending, b"", &mut out);
        assert!(out.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn flush_pending_strips_trailing_cr_on_unterminated_line() {
        let mut pending = b"x\r".to_vec();
        assert_eq!(flush_pending(&mut pending), Some("x".to_string()));
        assert!(pending.is_empty());
    }

    #[test]
    fn flush_pending_empty_returns_none() {
        let mut pending: Vec<u8> = Vec::new();
        assert_eq!(flush_pending(&mut pending), None);
    }

    #[test]
    fn flush_pending_without_trailing_cr() {
        let mut pending = b"plain".to_vec();
        assert_eq!(flush_pending(&mut pending), Some("plain".to_string()));
    }
}
