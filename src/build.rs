mod pty;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context as _;

#[derive(Debug, PartialEq, Eq)]
pub enum BuildDecision {
    Proceed,
    Review,
    Abort,
}

pub struct BuiltBase {
    pub artifacts: Vec<String>,
    pub version: Option<String>,
}

pub fn build_base(
    dir: &Path,
    package: &str,
    no_check: bool,
    on_line: impl FnMut(String),
) -> anyhow::Result<BuiltBase> {
    run_makepkg_streaming(dir, no_check, package, on_line)?;
    let expected = expected_artifacts(dir)
        .with_context(|| format!("failed to enumerate artifacts for {package}"))?;
    let artifacts = collect_artifacts(dir, &expected)?;
    let version = resolved_version(&expected, package);
    Ok(BuiltBase { artifacts, version })
}

fn collect_artifacts(dir: &Path, expected: &[String]) -> anyhow::Result<Vec<String>> {
    let artifacts = present_artifacts(dir, expected);
    if artifacts.is_empty() {
        anyhow::bail!("no built artifacts found in {}", dir.display());
    }
    Ok(artifacts)
}

fn present_artifacts(dir: &Path, expected: &[String]) -> Vec<String> {
    expected
        .iter()
        .filter_map(|basename| {
            let path = dir.join(basename);
            path.exists().then(|| path.to_string_lossy().into_owned())
        })
        .collect()
}

fn is_valid_pkgbase(s: &str) -> Option<()> {
    let first = s.as_bytes().first()?;
    (first.is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')))
    .then_some(())
}

pub fn clone_dir(pkgbase: &str) -> anyhow::Result<PathBuf> {
    if is_valid_pkgbase(pkgbase).is_none() {
        anyhow::bail!("invalid pkgbase from AUR: {pkgbase:?}");
    }
    Ok(crate::utils::cache_root()?.join("aur").join(pkgbase))
}

pub fn git_clone_or_pull(dir: &Path, pkgbase: &str) -> anyhow::Result<()> {
    if dir.exists() {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["pull", "--ff-only"])
            .stdout(Stdio::null())
            .status()
            .with_context(|| format!("failed to run git pull for {pkgbase}"))?;
        if !status.success() {
            anyhow::bail!("git pull failed for {pkgbase}");
        }
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = format!("https://aur.archlinux.org/{pkgbase}.git");
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1"])
        .arg(&url)
        .arg(dir)
        .stdout(Stdio::null())
        .status()
        .with_context(|| format!("failed to run git clone for {pkgbase}"))?;
    if !status.success() {
        anyhow::bail!("git clone failed for {pkgbase}");
    }
    Ok(())
}

fn expected_artifacts(dir: &Path) -> anyhow::Result<Vec<String>> {
    let output = std::process::Command::new("makepkg")
        .arg("--packagelist")
        .current_dir(dir)
        .output()
        .context("failed to run makepkg --packagelist")?;
    if !output.status.success() {
        anyhow::bail!("makepkg --packagelist exited {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let basename = Path::new(line.trim())
                .file_name()?
                .to_string_lossy()
                .into_owned();
            (!basename.is_empty()).then_some(basename)
        })
        .collect())
}

pub(crate) fn parse_package_filename(basename: &str) -> Option<(String, String)> {
    let mut parts = basename.rsplitn(4, '-');
    let (_arch, release, version, pkgname) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    Some((pkgname.to_string(), format!("{version}-{release}")))
}

fn resolved_version(expected: &[String], package: &str) -> Option<String> {
    let mut fallback = None;
    for basename in expected {
        let Some((pkgname, version)) = parse_package_filename(basename) else {
            continue;
        };
        if pkgname == package {
            return Some(version);
        }
        if fallback.is_none() {
            fallback = Some(version);
        }
    }
    fallback
}

pub fn run_makepkg_streaming(
    dir: &Path,
    no_check: bool,
    package: &str,
    mut on_line: impl FnMut(String),
) -> anyhow::Result<()> {
    let mut cmd = makepkg_command(dir, no_check);
    let mut session = pty::PtySession::spawn(&mut cmd)?;
    while let Some(line) = session.next_line()? {
        on_line(line);
    }
    let status = session.wait()?;
    if !status.success() {
        anyhow::bail!(
            "makepkg failed for {package} (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn makepkg_command(dir: &Path, no_check: bool) -> std::process::Command {
    let mut cmd = std::process::Command::new("makepkg");
    cmd.args(["--noconfirm", "-f"]);
    if no_check {
        cmd.arg("--nocheck");
    }
    cmd.current_dir(dir)
        .env("PKGDEST", dir)
        .stdin(Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_pkgbase_table() {
        let valid = ["google-chrome", "a.b+c-d", "1", "A", "abc-def_ghi+jkl.mno"];
        let invalid = ["", "../x", "-r", "a/b", "a b", "--global", ".hidden", "a:b"];
        for input in valid {
            assert!(
                is_valid_pkgbase(input).is_some(),
                "is_valid_pkgbase({input:?})"
            );
        }
        for input in invalid {
            assert!(
                is_valid_pkgbase(input).is_none(),
                "is_valid_pkgbase({input:?})"
            );
        }
    }

    #[test]
    fn parse_package_filename_table() {
        let cases = [
            (
                "cava-git-r1136.20a5997-2-x86_64.pkg.tar.zst",
                Some(("cava-git", "r1136.20a5997-2")),
            ),
            (
                "python-tokenize-rt-5.2.0-3-any.pkg.tar.zst",
                Some(("python-tokenize-rt", "5.2.0-3")),
            ),
            ("foo-1.pkg.tar.zst", None),
            ("foo-1-2", None),
        ];
        for (basename, expected) in cases {
            let parsed = parse_package_filename(basename);
            match expected {
                Some((name, version)) => {
                    let (pkgname, pkgver) = parsed.expect("parseable basename");
                    assert_eq!(pkgname, name);
                    assert_eq!(pkgver, version);
                }
                None => assert!(parsed.is_none(), "rejects {basename:?}"),
            }
        }
    }

    #[test]
    fn resolved_version_prefers_match_then_first_parseable() {
        let expected = vec![
            "too-few-segments.pkg.tar.zst".to_string(),
            "bar-1.0-1-x86_64.pkg.tar.zst".to_string(),
            "foo-2.5-1-x86_64.pkg.tar.zst".to_string(),
        ];
        assert_eq!(resolved_version(&expected, "foo").as_deref(), Some("2.5-1"));
        assert_eq!(
            resolved_version(&expected, "missing").as_deref(),
            Some("1.0-1")
        );
        assert!(resolved_version(&["junk".to_string()], "foo").is_none());
    }

    #[test]
    fn collect_artifacts_skips_missing_expected_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pipes-1.5-1-x86_64.pkg.tar.zst"), b"pkg")
            .expect("touch real artifact");
        let expected = vec![
            "pipes-1.5-1-x86_64.pkg.tar.zst".to_string(),
            "pipes-doc-1.5-1-x86_64.pkg.tar.zst".to_string(),
        ];
        let collected = collect_artifacts(dir.path(), &expected).expect("collect");
        assert_eq!(collected.len(), 1);
        assert!(collected[0].ends_with("pipes-1.5-1-x86_64.pkg.tar.zst"));
    }

    #[test]
    fn collect_artifacts_errors_when_nothing_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = vec![
            "pipes-1.5-1-x86_64.pkg.tar.zst".to_string(),
            "pipes-doc-1.5-1-x86_64.pkg.tar.zst".to_string(),
        ];
        let err = collect_artifacts(dir.path(), &expected).expect_err("must fail");
        assert!(err.to_string().contains("no built artifacts found"));
    }

    #[test]
    fn collect_artifacts_errors_for_empty_expected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = collect_artifacts(dir.path(), &[]).expect_err("must fail");
        assert!(err.to_string().contains("no built artifacts found"));
    }
}
