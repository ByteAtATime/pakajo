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

pub(crate) fn refresh_baseline(build_dir: &std::path::Path, arch: &str) -> anyhow::Result<()> {
    let srcinfo = if build_dir.join(".SRCINFO").exists() {
        crate::srcinfo_io::read_from_dir(build_dir)?
    } else {
        crate::srcinfo_io::generate(build_dir)?
    };
    let pkg_info = fetch_devel_info(arch, &srcinfo)?;
    if pkg_info.repos.is_empty() {
        return Ok(());
    }
    let mut devel = load_devel_info();
    merge_baseline(&mut devel, &srcinfo, pkg_info);
    save_devel_info(&devel)
}

fn merge_baseline(devel: &mut DevelInfo, srcinfo: &srcinfo::Srcinfo, pkg_info: PkgInfo) {
    for name in srcinfo.pkgnames() {
        if devel.info.contains_key(name) {
            devel.info.insert(name.to_string(), pkg_info.clone());
        }
    }
}

pub(crate) fn possible_devel_updates_from(info: &DevelInfo) -> Vec<String> {
    let targets: Vec<(&String, &RepoInfo)> = info
        .info
        .iter()
        .flat_map(|(pkgname, pkg)| pkg.repos.iter().map(move |repo| (pkgname, repo)))
        .collect();

    let results: Vec<Option<&String>> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(pkgname, repo)| {
                s.spawn(move || match crate::git::ls_remote(&repo.url, repo.branch.as_deref()) {
                    Ok(current) => (current != repo.commit).then_some(*pkgname),
                    Err(e) => {
                        eprintln!("warning: failed to look up {}: {e:#}", repo.url);
                        None
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(None))
            .collect()
    });

    let mut updated: Vec<String> = results
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>()
        .into_iter()
        .cloned()
        .collect();
    updated.sort();
    updated
}

pub(crate) fn possible_devel_updates() -> Vec<String> {
    possible_devel_updates_from(&load_devel_info())
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

    #[test]
    fn merge_baseline_updates_stale_commit() {
        let mut devel = DevelInfo::default();
        devel.info.insert(
            "example-git".to_string(),
            PkgInfo {
                repos: std::iter::once(RepoInfo {
                    url: "https://example.com/example.git".to_string(),
                    branch: None,
                    commit: "0000000000000000000000000000000000000000".to_string(),
                })
                .collect(),
            },
        );

        let srcinfo: ::srcinfo::Srcinfo = "pkgbase = example\n\
             pkgver = 1.0\n\
             pkgrel = 1\n\
             \n\
             pkgname = example-git\n"
            .parse()
            .expect("srcinfo parses");

        let fresh = PkgInfo {
            repos: std::iter::once(RepoInfo {
                url: "https://example.com/example.git".to_string(),
                branch: None,
                commit: "1111111111111111111111111111111111111111".to_string(),
            })
            .collect(),
        };

        merge_baseline(&mut devel, &srcinfo, fresh);

        let updated = devel.info.get("example-git").expect("entry remains");
        let repo = updated.repos.iter().next().expect("repo present");
        assert_eq!(repo.commit, "1111111111111111111111111111111111111111");
    }

    #[test]
    fn merge_baseline_creates_no_stray_entries() {
        let mut devel = DevelInfo::default();
        let srcinfo: ::srcinfo::Srcinfo = "pkgbase = example\n\
             pkgver = 1.0\n\
             pkgrel = 1\n\
             \n\
             pkgname = example-git\n"
            .parse()
            .expect("srcinfo parses");
        let fresh = PkgInfo {
            repos: std::iter::once(RepoInfo {
                url: "https://example.com/example.git".to_string(),
                branch: None,
                commit: "1111111111111111111111111111111111111111".to_string(),
            })
            .collect(),
        };

        merge_baseline(&mut devel, &srcinfo, fresh);

        assert!(devel.info.is_empty());
    }

    #[test]
    #[ignore]
    fn possible_devel_updates_from_detects_wrong_commit() {
        let mut info = DevelInfo::default();
        let pkg = PkgInfo {
            repos: std::iter::once(RepoInfo {
                url: "https://github.com/karlstav/cava.git".to_string(),
                branch: None,
                commit: "0000000000000000000000000000000000000000".to_string(),
            })
            .collect(),
        };
        info.info.insert("cava-git".to_string(), pkg);

        let updates = possible_devel_updates_from(&info);
        assert!(
            updates.iter().any(|name| name == "cava-git"),
            "expected cava-git to be reported as updatable, got {updates:?}"
        );
    }
}
