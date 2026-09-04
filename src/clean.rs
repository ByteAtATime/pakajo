use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

pub fn run_clean(remove: bool) -> anyhow::Result<()> {
    let root = crate::utils::cache_root()?;
    if !root.exists() {
        println!("cache directory not found: {}", root.display());
        return Ok(());
    }
    let clones = collect_clone_dirs(&root)?;
    if clones.is_empty() {
        println!("no clones to clean in {}", root.display());
        return Ok(());
    }
    let mut count = 0usize;
    for dir in &clones {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        if remove {
            match fs::remove_dir_all(dir) {
                Ok(()) => {
                    println!("removed {name}");
                    count += 1;
                }
                Err(e) => eprintln!("warning: failed to remove {name}: {e}"),
            }
        } else {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["clean", "-fdx"])
                .stdout(std::process::Stdio::null())
                .status()
                .with_context(|| format!("failed to spawn git for {name}"));
            match status {
                Ok(s) if s.success() => {
                    println!("cleaned {name}");
                    count += 1;
                }
                Ok(s) => eprintln!("warning: git clean failed for {name} (exit {})", s),
                Err(e) => eprintln!("warning: {e}"),
            }
        }
    }
    let verb = if remove { "removed" } else { "cleaned" };
    println!("{verb} {count} clone(s)");
    Ok(())
}

fn collect_clone_dirs(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join(".git").is_dir() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::collect_clone_dirs;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn touch(path: PathBuf) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, b"").unwrap();
    }

    #[test]
    fn collect_clone_dirs_filters_to_git_clones() {
        let root = TempDir::new().unwrap();
        let root_path = root.path();

        fs::create_dir_all(root_path.join("foo").join(".git")).unwrap();
        fs::create_dir_all(root_path.join("bar")).unwrap();
        touch(root_path.join("baz.txt"));
        touch(root_path.join("has-file").join(".git"));

        let result = collect_clone_dirs(root_path).unwrap();

        assert_eq!(
            result.len(),
            1,
            "expected exactly one clone, got {result:?}"
        );
        assert!(
            result[0].ends_with("foo"),
            "expected path ending in foo, got {}",
            result[0].display()
        );
    }

    #[test]
    fn collect_clone_dirs_empty_root_is_ok() {
        let root = TempDir::new().unwrap();
        let result = collect_clone_dirs(root.path()).unwrap();
        assert!(result.is_empty());
    }
}
