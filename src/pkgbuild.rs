use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context as _;

#[derive(Clone)]
pub struct PkgbuildInfo {
    pub name: String,
    pub pkgbase: String,
    pub dir: PathBuf,
    pub is_new: bool,
    pub needs_review: bool,
}

#[derive(Clone, Debug)]
pub struct PkgbuildDiff {
    pub name: String,
    pub pkgbase: String,
    pub dir: PathBuf,
    pub is_new: bool,
    pub diff: String,
}

struct NullSink;
impl crate::events::InstallSink for NullSink {
    fn event(&mut self, _event: crate::events::InstallEvent) {}
}

pub fn prepare_pkgbuild_diffs(targets: &[String]) -> anyhow::Result<Vec<PkgbuildDiff>> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let alpm = crate::pacman::init_alpm(&config)?;
    let aur = crate::aur::AurClient::new();
    let plan = crate::resolve::resolve(&crate::resolve::AlpmDb(&alpm), &aur, targets, false)?;
    let mut sink = NullSink;
    let pkgbuilds = collect_for_review(&plan, &mut sink)?;
    let diffs: Vec<PkgbuildDiff> = pkgbuilds
        .iter()
        .filter(|p| p.needs_review)
        .filter_map(|p| {
            let diff = compute_diff(&p.dir, p.is_new, false).ok()?;
            if diff.is_empty() {
                return None;
            }
            Some(PkgbuildDiff {
                name: p.name.clone(),
                pkgbase: p.pkgbase.clone(),
                dir: p.dir.clone(),
                is_new: p.is_new,
                diff,
            })
        })
        .collect();
    Ok(diffs)
}

pub fn has_seen_ref(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg("AUR_SEEN")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run git update-ref")?;
    if !status.success() {
        anyhow::bail!("git update-ref AUR_SEEN HEAD failed");
    }
    Ok(())
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
        let is_new = !has_seen_ref(&dir);
        let needs_review = has_diff(&dir);
        result.push(PkgbuildInfo {
            name: info.name.clone(),
            pkgbase: info.package_base.clone(),
            dir,
            is_new,
            needs_review,
        });
    }
    Ok(result)
}

const EMPTY_TREE_HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn compute_diff(dir: &Path, is_new: bool, use_color: bool) -> anyhow::Result<String> {
    let baseline = if is_new { EMPTY_TREE_HASH } else { "AUR_SEEN" };
    let color_arg = if use_color {
        "--color=always"
    } else {
        "--color=never"
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("--no-pager")
        .arg("diff")
        .arg(color_arg)
        .arg(format!("{baseline}..HEAD"))
        .arg("--")
        .arg(".")
        .arg(":(exclude).SRCINFO")
        .output()
        .context("failed to run git diff")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed in {}: {}", dir.display(), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        assert!(
            status.success(),
            "git {:?} failed in {}",
            args,
            dir.display()
        );
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

    #[test]
    fn test_compute_diff_shows_full_content_for_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        init_repo(path);
        fs::write(path.join("PKGBUILD"), "pkgname=foo\npkgver=1\n").unwrap();
        fs::write(path.join(".SRCINFO"), "should be excluded\n").unwrap();
        commit_all(path, "init");

        assert!(!has_seen_ref(path));
        let diff = compute_diff(path, true, false).unwrap();
        assert!(
            diff.contains("pkgname=foo"),
            "diff should contain PKGBUILD content"
        );
        assert!(diff.contains("pkgver=1"), "diff should contain pkgver");
        assert!(
            !diff.contains("should be excluded"),
            ".SRCINFO must be excluded"
        );
    }

    #[test]
    fn test_compute_diff_shows_changes_after_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        init_repo(path);
        fs::write(path.join("PKGBUILD"), "pkgver=1\n").unwrap();
        commit_all(path, "init");
        mark_seen(path).unwrap();

        fs::write(path.join("PKGBUILD"), "pkgver=2\n").unwrap();
        commit_all(path, "bump");

        let diff = compute_diff(path, false, false).unwrap();
        assert!(
            diff.contains("pkgver=1"),
            "diff should show old version being removed"
        );
        assert!(
            diff.contains("pkgver=2"),
            "diff should show new version being added"
        );
    }
}
