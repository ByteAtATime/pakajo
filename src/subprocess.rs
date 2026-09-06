use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};

use anyhow::Context as _;
use futures::SinkExt as _;
use futures::channel::mpsc;

use crate::cli::{escalation_command, graphical_escalation_command};
use crate::events::{InstallEvent, InstallSink, read_event_stream};
use crate::install::{ChildOutcome, StreamItem, map_outcome};

pub enum ChildJob {
    Install {
        targets: Vec<String>,
        as_deps: bool,
        approvals_b64: Option<String>,
    },
    Remove {
        targets: Vec<String>,
    },
    Upgrade {
        no_refresh: bool,
        ignores: Vec<String>,
        fingerprint_file: Option<String>,
        approvals_b64: Option<String>,
    },
}

impl ChildJob {
    fn subcommand(&self) -> &'static str {
        match self {
            ChildJob::Install { .. } => "install",
            ChildJob::Remove { .. } => "remove",
            ChildJob::Upgrade { .. } => "upgrade",
        }
    }

    fn apply(&self, cmd: &mut Command) {
        cmd.arg(self.subcommand()).arg("--json");
        match self {
            ChildJob::Install {
                targets,
                as_deps,
                approvals_b64,
            } => {
                if *as_deps {
                    cmd.arg("--asdeps");
                }
                if let Some(b64) = approvals_b64 {
                    cmd.arg("--approvals").arg(b64);
                }
                for target in targets {
                    cmd.arg(target);
                }
            }
            ChildJob::Remove { targets } => {
                for target in targets {
                    cmd.arg(target);
                }
            }
            ChildJob::Upgrade {
                no_refresh,
                ignores,
                fingerprint_file,
                approvals_b64,
            } => {
                cmd.arg("--repo-only");
                if *no_refresh {
                    cmd.arg("--no-refresh");
                }
                for name in ignores {
                    cmd.arg("--ignore").arg(name);
                }
                if let Some(file) = fingerprint_file {
                    cmd.arg("--fingerprint-file").arg(file);
                }
                if let Some(b64) = approvals_b64 {
                    cmd.arg("--approvals").arg(b64);
                }
            }
        }
    }
}

pub fn spawn_escalated(job: &ChildJob, stdin: Stdio) -> anyhow::Result<Child> {
    let exe = std::env::current_exe().context("failed to determine executable path")?;
    let mut cmd = escalation_command(&exe.to_string_lossy());
    job.apply(&mut cmd);
    cmd.stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd.spawn()
        .with_context(|| format!("failed to spawn {} child", job.subcommand()))
}

pub fn send_item(tx: &mut mpsc::Sender<StreamItem>, item: StreamItem) {
    futures::executor::block_on(tx.send(item)).ok();
}

pub struct ChannelSink {
    tx: mpsc::Sender<StreamItem>,
}

impl ChannelSink {
    pub fn new(tx: mpsc::Sender<StreamItem>) -> Self {
        Self { tx }
    }
}

impl InstallSink for ChannelSink {
    fn event(&mut self, event: InstallEvent) {
        send_item(&mut self.tx, StreamItem::Event(event));
    }
}

pub fn stream_child<S: InstallSink + ?Sized>(
    mut child: Child,
    sink: &mut S,
) -> std::io::Result<ExitStatus> {
    let stdout = child.stdout.take().expect("piped stdout");
    read_event_stream(BufReader::new(stdout), sink);
    Ok(child.wait()?)
}

pub fn run_job_to_channel(exe: PathBuf, job: ChildJob, mut tx: mpsc::Sender<StreamItem>) {
    let mut cmd = graphical_escalation_command(&exe.to_string_lossy());
    job.apply(&mut cmd);
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

    fn args_of(job: &ChildJob) -> Vec<String> {
        let mut cmd = Command::new("pakajo");
        job.apply(&mut cmd);
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn install_job_matches_cli_vector() {
        let job = ChildJob::Install {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            as_deps: true,
            approvals_b64: Some("QUJD".to_string()),
        };
        assert_eq!(
            args_of(&job),
            [
                "install",
                "--json",
                "--asdeps",
                "--approvals",
                "QUJD",
                "sl",
                "figlet"
            ]
        );
    }

    #[test]
    fn install_job_minimal_matches_gui_vector() {
        let job = ChildJob::Install {
            targets: vec!["sl".to_string()],
            as_deps: false,
            approvals_b64: None,
        };
        assert_eq!(args_of(&job), ["install", "--json", "sl"]);
    }

    #[test]
    fn remove_job_matches_vector() {
        let job = ChildJob::Remove {
            targets: vec!["sl".to_string(), "figlet".to_string()],
        };
        assert_eq!(args_of(&job), ["remove", "--json", "sl", "figlet"]);
    }

    #[test]
    fn upgrade_job_matches_cli_vector() {
        let job = ChildJob::Upgrade {
            no_refresh: true,
            ignores: vec!["foo".to_string(), "bar".to_string()],
            fingerprint_file: None,
            approvals_b64: None,
        };
        assert_eq!(
            args_of(&job),
            [
                "upgrade",
                "--json",
                "--repo-only",
                "--no-refresh",
                "--ignore",
                "foo",
                "--ignore",
                "bar"
            ]
        );
    }

    #[test]
    fn upgrade_job_matches_gui_vector() {
        let job = ChildJob::Upgrade {
            no_refresh: false,
            ignores: vec![],
            fingerprint_file: Some("/tmp/fp.json".to_string()),
            approvals_b64: Some("QUJD".to_string()),
        };
        assert_eq!(
            args_of(&job),
            [
                "upgrade",
                "--json",
                "--repo-only",
                "--fingerprint-file",
                "/tmp/fp.json",
                "--approvals",
                "QUJD"
            ]
        );
    }
}
