use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

#[derive(Clone)]
pub struct PkgbuildInfo {
    pub name: String,
    pub pkgbase: String,
    pub dir: PathBuf,
    pub commit: String,
    pub is_new: bool,
    pub needs_review: bool,
}

pub fn has_seen_ref(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg("AUR_SEEN")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn has_diff(dir: &Path) -> bool {
    if !has_seen_ref(dir) {
        return true;
    }
    let output = match Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("AUR_SEEN")
        .arg("HEAD")
        .output()
    {
        Ok(o) => o,
        Err(_) => return true,
    };
    if !output.status.success() {
        return true;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    match (lines.next(), lines.next()) {
        (Some(seen), Some(head)) => seen != head,
        _ => true,
    }
}

pub fn mark_seen(dir: &Path) -> anyhow::Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("update-ref")
        .arg("AUR_SEEN")
        .arg("HEAD")
        .status()
        .context("failed to run git update-ref")?;
    if !status.success() {
        anyhow::bail!("git update-ref AUR_SEEN HEAD failed");
    }
    Ok(())
}

pub fn current_commit(dir: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .context("failed to run git rev-parse HEAD")?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn collect_for_review<S: crate::events::InstallSink + ?Sized>(
    plan: &crate::resolve::BuildPlan,
    sink: &mut S,
) -> anyhow::Result<Vec<PkgbuildInfo>> {
    let mut result = Vec::new();
    for info in plan.layers.iter().flat_map(|l| l.aur.iter()) {
        let dir = crate::build::clone_dir(&info.package_base)?;
        sink.event(crate::events::InstallEvent::CloningRepo {
            package: info.name.clone(),
        });
        crate::build::git_clone_or_pull(&dir, &info.package_base)?;
        let commit = current_commit(&dir).unwrap_or_default();
        let is_new = !has_seen_ref(&dir);
        let needs_review = has_diff(&dir);
        result.push(PkgbuildInfo {
            name: info.name.clone(),
            pkgbase: info.package_base.clone(),
            dir,
            commit,
            is_new,
            needs_review,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args);
        cmd.env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test");
        let status = cmd.status().expect("git command runs");
        assert!(status.success(), "git {:?} failed in {}", args, dir.display());
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
    }

    fn commit_all(dir: &Path, msg: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn test_mark_seen_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        init_repo(path);
        fs::write(path.join("a.txt"), "a").unwrap();
        commit_all(path, "init");

        assert!(!has_seen_ref(path));
        mark_seen(path).unwrap();
        assert!(has_seen_ref(path));
    }

    #[test]
    fn test_has_diff_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        init_repo(path);
        fs::write(path.join("a.txt"), "a").unwrap();
        commit_all(path, "init");
        mark_seen(path).unwrap();
        assert!(!has_diff(path));

        fs::write(path.join("a.txt"), "b").unwrap();
        commit_all(path, "change");
        assert!(has_diff(path));

        mark_seen(path).unwrap();
        assert!(!has_diff(path));
    }

    #[test]
    fn test_has_diff_never_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        init_repo(path);
        fs::write(path.join("a.txt"), "a").unwrap();
        commit_all(path, "init");

        assert!(!has_seen_ref(path));
        assert!(has_diff(path));
    }
}
