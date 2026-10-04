use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CancelTarget {
    Process(i32),
    Group(i32),
}

#[derive(Debug, Default)]
struct Inner {
    requested: AtomicBool,
    targets: Mutex<Vec<CancelTarget>>,
}

#[derive(Clone, Debug, Default)]
pub struct CancelHandle {
    inner: Arc<Inner>,
}

impl CancelHandle {
    pub fn is_requested(&self) -> bool {
        self.inner.requested.load(Ordering::SeqCst)
    }

    pub fn register(&self, target: CancelTarget) {
        self.targets_locked().push(target.clone());
        if self.is_requested() {
            deliver(&target);
        }
    }

    pub fn clear(&self, target: &CancelTarget) {
        let mut targets = self.targets_locked();
        if let Some(index) = targets.iter().position(|entry| entry == target) {
            targets.remove(index);
        }
    }

    pub fn cancel(&self) {
        self.inner.requested.store(true, Ordering::SeqCst);
        for target in self.targets_locked().clone() {
            deliver(&target);
        }
    }

    fn targets_locked(&self) -> std::sync::MutexGuard<'_, Vec<CancelTarget>> {
        self.inner
            .targets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn deliver(target: &CancelTarget) {
    match *target {
        CancelTarget::Process(pid) => {
            if pid > 1 {
                unsafe {
                    libc::kill(pid, libc::SIGINT);
                }
            }
        }
        CancelTarget::Group(pgid) => {
            if pgid > 1 {
                unsafe {
                    libc::kill(-pgid, libc::SIGINT);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn sleep_child() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep")
    }

    fn wait_for_exit(child: &mut std::process::Child, what: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("poll sleep") {
                Some(status) => return status,
                None => {
                    if Instant::now() > deadline {
                        panic!("{what} did not exit in time");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    fn kill_child(child: &mut std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn cancel_interrupts_registered_child() {
        let handle = CancelHandle::default();
        let mut child = sleep_child();
        handle.register(CancelTarget::Process(child.id() as i32));
        handle.cancel();
        let status = wait_for_exit(&mut child, "cancelled sleep");
        assert!(handle.is_requested());
        assert_eq!(status.signal(), Some(libc::SIGINT));
    }

    #[test]
    fn cleared_child_survives_cancel() {
        let handle = CancelHandle::default();
        let mut child = sleep_child();
        let target = CancelTarget::Process(child.id() as i32);
        handle.register(target.clone());
        handle.clear(&target);
        handle.cancel();
        std::thread::sleep(Duration::from_millis(200));
        assert!(child.try_wait().expect("poll sleep").is_none());
        kill_child(&mut child);
    }

    #[test]
    fn late_registration_dies_at_birth() {
        let handle = CancelHandle::default();
        handle.cancel();
        let mut child = sleep_child();
        handle.register(CancelTarget::Process(child.id() as i32));
        let status = wait_for_exit(&mut child, "late-registered sleep");
        assert_eq!(status.signal(), Some(libc::SIGINT));
    }

    #[test]
    fn cancel_reaches_process_group() {
        let handle = CancelHandle::default();
        let mut child = grouped_sleep_child();
        handle.register(CancelTarget::Group(child.id() as i32));
        handle.cancel();
        let status = wait_for_exit(&mut child, "grouped sleep");
        assert_eq!(status.signal(), Some(libc::SIGINT));
    }

    fn grouped_sleep_child() -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        unsafe {
            Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
                .spawn()
                .expect("spawn grouped sleep")
        }
    }

    #[test]
    fn cancel_reaches_group_and_process_targets_together() {
        let handle = CancelHandle::default();
        let mut grouped = grouped_sleep_child();
        let mut plain = sleep_child();
        handle.register(CancelTarget::Group(grouped.id() as i32));
        handle.register(CancelTarget::Process(plain.id() as i32));
        handle.cancel();
        let grouped_status = wait_for_exit(&mut grouped, "grouped sleep");
        let plain_status = wait_for_exit(&mut plain, "plain sleep");
        assert_eq!(grouped_status.signal(), Some(libc::SIGINT));
        assert_eq!(plain_status.signal(), Some(libc::SIGINT));
    }

    #[test]
    fn cleared_process_target_survives_group_cancel() {
        let handle = CancelHandle::default();
        let mut grouped = grouped_sleep_child();
        let mut plain = sleep_child();
        let plain_target = CancelTarget::Process(plain.id() as i32);
        handle.register(CancelTarget::Group(grouped.id() as i32));
        handle.register(plain_target.clone());
        handle.clear(&plain_target);
        handle.cancel();
        let grouped_status = wait_for_exit(&mut grouped, "grouped sleep");
        assert_eq!(grouped_status.signal(), Some(libc::SIGINT));
        std::thread::sleep(Duration::from_millis(200));
        assert!(plain.try_wait().expect("poll sleep").is_none());
        kill_child(&mut plain);
    }
}
