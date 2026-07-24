use std::collections::{HashMap, HashSet};

use anyhow::Context as _;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RepoInfo {
    pub url: String,
    pub branch: Option<String>,
    pub commit: String,
}

impl std::hash::Hash for RepoInfo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.url.hash(state);
        self.branch.hash(state);
    }
}

impl PartialEq for RepoInfo {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.branch == other.branch
    }
}

impl Eq for RepoInfo {}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PkgInfo {
    pub repos: HashSet<RepoInfo>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DevelInfo {
    pub info: HashMap<String, PkgInfo>,
}

pub(crate) fn state_path() -> std::path::PathBuf {
    cache_dir().join("pakajo").join("devel.json")
}

fn cache_dir() -> std::path::PathBuf {
    match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => std::path::PathBuf::from(xdg),
        Err(_) => {
            let home = std::env::var("HOME").expect("no cache directory: set XDG_CACHE_HOME or HOME");
            std::path::PathBuf::from(home).join(".cache")
        }
    }
}

pub(crate) fn load_devel_info() -> DevelInfo {
    load_from(&state_path())
}

fn load_from(path: &std::path::Path) -> DevelInfo {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return DevelInfo::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub(crate) fn save_devel_info(info: &DevelInfo) -> anyhow::Result<()> {
    save_to(info, &state_path())
}

fn save_to(info: &DevelInfo, path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let serialized = serde_json::to_string_pretty(info).context("failed to serialize devel info")?;
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn parse_url(source: &str) -> Option<(String, Option<String>)> {
    let url = source.splitn(2, "::").last().unwrap();

    if !url.starts_with("git") || !url.contains("://") {
        return None;
    }

    let mut split = url.splitn(2, "://");
    let protocol = split.next().unwrap();
    let protocol = protocol.rsplit('+').next().unwrap();
    let rest = split.next().unwrap();

    let mut split = rest.splitn(2, '#');
    let remote = split.next().unwrap();
    let remote = remote.split_once('?').map_or(remote, |(x, _)| x);
    let remote = format!("{}://{}", protocol, remote);

    let branch = if let Some(fragment) = split.next() {
        let fragment = fragment.split_once('?').map_or(fragment, |(x, _)| x);
        let mut split = fragment.splitn(2, '=');
        let frag_type = split.next().unwrap();
        match frag_type {
            "commit" | "tag" => return None,
            "branch" => split.next().map(str::to_string),
            _ => None,
        }
    } else {
        None
    };

    Some((remote, branch))
}

pub(crate) fn fetch_devel_info(arch: &str, srcinfo: &srcinfo::Srcinfo) -> anyhow::Result<PkgInfo> {
    let mut targets: Vec<(String, Option<String>)> = Vec::new();
    for source in srcinfo.base.source.arch(arch) {
        if let Some((url, branch)) = parse_url(source) {
            targets.push((url, branch));
        }
    }

    let results: Vec<Option<RepoInfo>> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(url, branch)| {
                s.spawn(move || match crate::git::ls_remote(url, branch.as_deref()) {
                    Ok(commit) => Some(RepoInfo {
                        url: url.clone(),
                        branch: branch.clone(),
                        commit,
                    }),
                    Err(e) => {
                        eprintln!("warning: failed to look up {url}: {e:#}");
                        None
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut repos = HashSet::new();
    for repo in results.into_iter().flatten() {
        repos.replace(repo);
    }
    Ok(PkgInfo { repos })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_git_remote() {
        assert_eq!(
            parse_url("git+https://example.com/repo.git"),
            Some(("https://example.com/repo.git".to_string(), None))
        );
    }

    #[test]
    fn parses_branch_fragment() {
        assert_eq!(
            parse_url("git+https://example.com/repo.git#branch=main"),
            Some(("https://example.com/repo.git".to_string(), Some("main".to_string())))
        );
    }

    #[test]
    fn rejects_commit_fragment() {
        assert_eq!(parse_url("git+https://example.com/repo.git#commit=abc"), None);
    }

    #[test]
    fn rejects_tag_fragment() {
        assert_eq!(parse_url("git+https://example.com/repo.git#tag=v1"), None);
    }

    #[test]
    fn strips_rename_prefix() {
        assert_eq!(
            parse_url("name::git+https://example.com/repo.git"),
            Some(("https://example.com/repo.git".to_string(), None))
        );
    }

    #[test]
    fn ignores_non_git_source() {
        assert_eq!(parse_url("https://example.com/file.tar.gz"), None);
    }

    #[test]
    fn strips_query_suffix() {
        assert_eq!(
            parse_url("git+https://example.com/repo.git?signed"),
            Some(("https://example.com/repo.git".to_string(), None))
        );
    }

    #[test]
    fn load_from_missing_path_returns_empty() {
        let dir = std::env::temp_dir().join(format!(
            "pakajo_devel_missing_{}_{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("devel.json");
        let info = load_from(&path);
        assert!(info.info.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "pakajo_devel_rt_{}_{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("devel.json");

        let mut info = DevelInfo::default();
        let pkg = PkgInfo {
            repos: std::iter::once(RepoInfo {
                url: "https://example.com/repo.git".to_string(),
                branch: Some("main".to_string()),
                commit: "abc123".to_string(),
            })
            .collect(),
        };
        info.info.insert("mypackage-git".to_string(), pkg);

        save_to(&info, &path).expect("save succeeds");
        let loaded = load_from(&path);

        assert_eq!(loaded.info.len(), 1);
        let loaded_pkg = loaded.info.get("mypackage-git").expect("pkg present");
        assert_eq!(loaded_pkg.repos.len(), 1);
        let repo = loaded_pkg.repos.iter().next().expect("repo present");
        assert_eq!(repo.url, "https://example.com/repo.git");
        assert_eq!(repo.branch.as_deref(), Some("main"));
        assert_eq!(repo.commit, "abc123");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_corrupt_file_returns_empty() {
        let dir = std::env::temp_dir().join(format!(
            "pakajo_devel_corrupt_{}_{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devel.json");
        std::fs::write(&path, b"{ not valid json").unwrap();

        let info = load_from(&path);
        assert!(info.info.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
