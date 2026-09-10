use std::process::{Child, Stdio};

use crate::subprocess::{ChildJob, spawn_escalated};

pub use crate::dispatch::exec::{escalation_command, graphical_escalation_command};

fn run_escalated_child(child: Child, json: bool) -> anyhow::Result<i32> {
    let mut sink = super::sink_for(json);
    let status = crate::subprocess::stream_child(child, &mut *sink)?;
    Ok(status.code().unwrap_or(1))
}

fn escalate_exit(result: anyhow::Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

pub fn escalate_upgrade(no_refresh: bool, ignores: &[String], json: bool) -> i32 {
    let result = spawn_escalated(
        &ChildJob::Upgrade {
            no_refresh,
            ignores: ignores.to_vec(),
            fingerprint_file: None,
            approvals_b64: None,
        },
        Stdio::inherit(),
    )
    .and_then(|child| run_escalated_child(child, json));
    escalate_exit(result)
}
