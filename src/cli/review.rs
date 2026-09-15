use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::Context as _;

use crate::cli::prompts::confirm_review_accept;
use crate::color;
use crate::pkgbuild::{PkgbuildInfo, compute_diff};

pub fn review_pkgbuilds(pkgbuilds: &[PkgbuildInfo]) -> bool {
    let use_color = color::stdout_color();
    let mut combined = String::new();
    for pb in pkgbuilds {
        match compute_diff(&pb.dir, pb.is_new, use_color) {
            Ok(diff) if diff.is_empty() => continue,
            Ok(diff) => {
                combined.push_str(&color::colon(use_color, &pb.name));
                combined.push_str("\n\n");
                combined.push_str(&diff);
                combined.push_str("\n\n");
            }
            Err(e) => {
                eprintln!("warning: failed to compute diff for {}: {e:#}", pb.name);
            }
        }
    }

    if combined.is_empty() {
        println!(
            "{}",
            color::colon(use_color, "Nothing new to review - all PKGBUILDs unchanged")
        );
        return true;
    }

    let pager = resolve_pager();
    let result = run_pager(&pager, &combined);
    if let Err(e) = result {
        eprintln!("warning: pager '{pager}' failed: {e:#}, printing to stdout");
        let _ = std::io::stdout().write_all(combined.as_bytes());
    }

    confirm_review_accept()
}

fn resolve_pager() -> String {
    if let Ok(p) = std::env::var("PAGER")
        && !p.is_empty()
    {
        return p;
    }
    if which("less") {
        "less".to_string()
    } else {
        "cat".to_string()
    }
}

fn which(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_pager(pager: &str, content: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(pager);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    if std::env::var("LESS").is_err() {
        cmd.env("LESS", "SRXF");
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn pager '{pager}'"))?;

    if let Some(stdin) = child.stdin.as_mut()
        && let Err(e) = stdin.write_all(content.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(anyhow::Error::from(e).context("failed to write to pager stdin"));
    }

    drop(child.stdin.take());

    let status = child.wait().context("failed to wait for pager")?;
    if !status.success() {
        eprintln!("warning: pager '{pager}' exited with non-zero status");
    }
    Ok(())
}
