use std::process::Stdio;
use std::time::Duration;

use anyhow::Context as _;
use wait_timeout::ChildExt as _;

pub(crate) fn ls_remote(url: &str, branch: Option<&str>) -> anyhow::Result<String> {
    let mut command = std::process::Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("ls-remote")
        .arg(url)
        .arg(branch.unwrap_or("HEAD"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run git ls-remote for {url}"))?;

    let timeout = Duration::from_secs(15);
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            if !status.success() {
                let stderr = child
                    .wait_with_output()
                    .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
                    .unwrap_or_default();
                anyhow::bail!("git ls-remote failed for {url}: {stderr}");
            }
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("git ls-remote timed out after 15s for {url}");
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e).with_context(|| format!("git ls-remote failed for {url}"));
        }
    }

    let output = child
        .wait_with_output()
        .context("failed to read git ls-remote output")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout.split('\t').next().unwrap_or("").trim().to_string();
    if sha.is_empty() {
        anyhow::bail!("git ls-remote returned no sha for {url}");
    }
    Ok(sha)
}

#[cfg(test)]
mod tests {
    use super::ls_remote;

    #[test]
    #[ignore]
    fn ls_remote_returns_hex_sha() {
        let sha = ls_remote("https://github.com/karlstav/cava.git", None)
            .expect("ls-remote should succeed for a stable public repo");
        assert!(
            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "expected a 40-char hex SHA, got {sha:?}"
        );
    }
}
