use std::fmt::Write as _;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use flate2::Compression;
use flate2::write::GzEncoder;

pub fn build_stub_pkg(name: &str, version: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let version = ensure_release(version);
    let mut pkginfo = String::new();
    writeln!(pkginfo, "pkgname = {name}")?;
    writeln!(pkginfo, "pkgver = {version}")?;
    writeln!(pkginfo, "arch = any")?;
    writeln!(pkginfo, "builddate = {}", now_unix())?;

    let filename = format!("{name}-{version}-any.pkg.tar.gz");
    let path = dir.join(&filename);
    let file = std::fs::File::create(&path).context("failed to create stub package file")?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);

    let bytes = pkginfo.into_bytes();
    let mut header = tar::Header::new_gnu();
    header
        .set_path(".PKGINFO")
        .context("failed to set tar header path")?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, &mut Cursor::new(&bytes))
        .context("failed to append .PKGINFO to tar archive")?;
    tar.finish().context("failed to finalize tar archive")?;

    Ok(path)
}

fn ensure_release(version: &str) -> String {
    if version.contains('-') {
        version.to_string()
    } else {
        format!("{version}-1")
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpm::SigLevel;

    fn test_handle() -> alpm::Alpm {
        let db = tempfile::tempdir().expect("temp db dir");
        alpm::Alpm::new("/", db.path().to_str().expect("utf8 temp path")).expect("alpm handle")
    }

    #[test]
    fn stub_pkg_round_trips() {
        let dir = tempfile::tempdir().expect("stub dir");
        let path = build_stub_pkg("cava-git", "0.10.4-1", dir.path()).expect("build stub");

        let handle = test_handle();
        let loaded = handle
            .pkg_load(path.to_string_lossy().as_ref(), false, SigLevel::NONE)
            .expect("pkg_load stub");
        assert_eq!(loaded.name(), "cava-git");
        assert_eq!(loaded.version().to_string(), "0.10.4-1");
        assert_eq!(loaded.arch(), Some("any"));
        assert!(loaded.depends().is_empty());
        assert!(loaded.conflicts().is_empty());
    }
}
