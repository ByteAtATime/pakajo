use std::collections::{HashMap, HashSet};

use anyhow::Context as _;

use crate::aur::AurInfo;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoInfo {
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
pub struct PkgInfo {
    pub repos: HashSet<RepoInfo>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DevelInfo {
    pub info: HashMap<String, PkgInfo>,
}

#[derive(serde::Deserialize)]
struct YayOriginInfo {
    #[serde(default)]
    protocols: Vec<String>,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    sha: String,
}

pub fn state_path() -> std::path::PathBuf {
    cache_dir().join("pakajo").join("devel.json")
}

fn cache_dir() -> std::path::PathBuf {
    match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => std::path::PathBuf::from(xdg),
        Err(_) => {
            let home =
                std::env::var("HOME").expect("no cache directory: set XDG_CACHE_HOME or HOME");
            std::path::PathBuf::from(home).join(".cache")
        }
    }
}

pub fn load_devel_info() -> DevelInfo {
    let path = state_path();
    if path.exists() {
        return load_from(&path);
    }
    if let Some(imported) = std::fs::read(yay_vcs_path())
        .ok()
        .and_then(|b| import_yay_from(&b))
    {
        let _ = save_to(&imported, &path);
        return imported;
    }
    DevelInfo::default()
}

fn yay_vcs_path() -> std::path::PathBuf {
    cache_dir().join("yay").join("vcs.json")
}

fn load_from(path: &std::path::Path) -> DevelInfo {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return DevelInfo::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn import_yay_from(bytes: &[u8]) -> Option<DevelInfo> {
    let yay: HashMap<String, HashMap<String, YayOriginInfo>> =
        serde_json::from_slice(bytes).ok()?;

    let mut info: HashMap<String, PkgInfo> = HashMap::new();
    for (pkgname, url_map) in yay {
        let mut repos: HashSet<RepoInfo> = HashSet::new();
        for (url_key, oi) in url_map {
            let Some(proto) = oi.protocols.last() else {
                continue;
            };
            if oi.sha.is_empty() {
                continue;
            }
            let url = format!("{proto}://{url_key}");
            let branch = if oi.branch.is_empty() || oi.branch == "HEAD" {
                None
            } else {
                Some(oi.branch.clone())
            };
            repos.replace(RepoInfo {
                url,
                branch,
                commit: oi.sha,
            });
        }
        if repos.is_empty() {
            continue;
        }
        info.insert(pkgname, PkgInfo { repos });
    }

    if info.is_empty() {
        return None;
    }
    Some(DevelInfo { info })
}

pub fn save_devel_info(info: &DevelInfo) -> anyhow::Result<()> {
    save_to(info, &state_path())
}

fn save_to(info: &DevelInfo, path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let serialized =
        serde_json::to_string_pretty(info).context("failed to serialize devel info")?;
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn parse_url(source: &str) -> Option<(String, Option<String>)> {
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

pub fn fetch_devel_info(arch: &str, srcinfo: &srcinfo::Srcinfo) -> anyhow::Result<PkgInfo> {
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
                s.spawn(
                    move || match crate::git::ls_remote(url, branch.as_deref()) {
                        Ok(commit) => Some(RepoInfo {
                            url: url.clone(),
                            branch: branch.clone(),
                            commit,
                        }),
                        Err(e) => {
                            eprintln!("warning: failed to look up {url}: {e:#}");
                            None
                        }
                    },
                )
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

pub fn refresh_baseline(build_dir: &std::path::Path, arch: &str) -> anyhow::Result<()> {
    let srcinfo = srcinfo_for_dir(build_dir)?;
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
        devel.info.insert(name.to_string(), pkg_info.clone());
    }
}

fn srcinfo_for_dir(dir: &std::path::Path) -> anyhow::Result<srcinfo::Srcinfo> {
    if dir.join(".SRCINFO").exists() {
        crate::srcinfo_io::read_from_dir(dir)
    } else {
        crate::srcinfo_io::generate(dir)
    }
}

fn fetch_base_devel_info(base: &str, arch: &str) -> anyhow::Result<Option<PkgInfo>> {
    let dir = crate::build::clone_dir(base)?;
    crate::build::git_clone_or_pull(&dir, base)?;
    let srcinfo = srcinfo_for_dir(&dir)?;
    let pkg_info = fetch_devel_info(arch, &srcinfo)?;
    if pkg_info.repos.is_empty() {
        return Ok(None);
    }
    Ok(Some(pkg_info))
}

fn group_by_base(infos: &[AurInfo]) -> std::collections::HashMap<String, Vec<String>> {
    let mut base_to_names: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for info in infos {
        base_to_names
            .entry(info.package_base.clone())
            .or_default()
            .push(info.name.clone());
    }
    base_to_names
}

fn record_devel_infos(devel: &mut DevelInfo, infos: &[AurInfo], arch: &str) -> usize {
    let base_to_names = group_by_base(infos);
    let mut recorded = 0usize;
    for (base, names) in &base_to_names {
        let pkg_info = match fetch_base_devel_info(base, arch) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("warning: skipping {base}: {e:#}");
                continue;
            }
        };
        for name in names {
            devel.info.insert(name.clone(), pkg_info.clone());
        }
        recorded += 1;
    }
    recorded
}

pub enum GendbOutcome {
    NoForeign,
    LookupFailed,
    Recorded(usize),
}

pub fn generate_db(
    handle: &alpm::Alpm,
    aur: &impl crate::resolve::AurQuery,
) -> anyhow::Result<GendbOutcome> {
    let arch = handle
        .architectures()
        .first()
        .context("no architecture configured in alpm")?;
    let foreign: Vec<String> = crate::package::foreign_names(handle);
    let mut devel = load_devel_info();
    if foreign.is_empty() {
        save_devel_info(&devel)?;
        return Ok(GendbOutcome::NoForeign);
    }
    let infos = match aur.info_many(&foreign) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("warning: AUR info lookup failed: {e:#}");
            save_devel_info(&devel)?;
            return Ok(GendbOutcome::LookupFailed);
        }
    };
    let recorded = record_devel_infos(&mut devel, &infos, arch);
    save_devel_info(&devel)?;
    Ok(GendbOutcome::Recorded(recorded))
}

pub fn possible_devel_updates_from(info: &DevelInfo) -> Vec<String> {
    let targets: Vec<(&String, &RepoInfo)> = info
        .info
        .iter()
        .flat_map(|(pkgname, pkg)| pkg.repos.iter().map(move |repo| (pkgname, repo)))
        .collect();

    let results: Vec<Option<&String>> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(pkgname, repo)| {
                s.spawn(
                    move || match crate::git::ls_remote(&repo.url, repo.branch.as_deref()) {
                        Ok(current) => (current != repo.commit).then_some(*pkgname),
                        Err(e) => {
                            eprintln!("warning: failed to look up {}: {e:#}", repo.url);
                            None
                        }
                    },
                )
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

pub fn possible_devel_updates() -> Vec<String> {
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
            Some((
                "https://example.com/repo.git".to_string(),
                Some("main".to_string())
            ))
        );
    }

    #[test]
    fn rejects_commit_fragment() {
        assert_eq!(
            parse_url("git+https://example.com/repo.git#commit=abc"),
            None
        );
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
    fn merge_baseline_auto_registers_new_package() {
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

        let registered = devel
            .info
            .get("example-git")
            .expect("package auto-registered");
        let repo = registered.repos.iter().next().expect("repo present");
        assert_eq!(repo.commit, "1111111111111111111111111111111111111111");
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

    fn find_repo<'a>(pkg: &'a PkgInfo, url: &str) -> &'a RepoInfo {
        pkg.repos
            .iter()
            .find(|r| r.url == url)
            .expect("repo present")
    }

    #[test]
    fn import_yay_from_converts_happy_path() {
        let bytes = br#"{"cava-git":{"github.com/karlstav/cava.git":{"protocols":["https"],"branch":"HEAD","sha":"abc"}},"foo-git":{"github.com/x/foo.git":{"protocols":["ssh"],"branch":"dev","sha":"def"}}}"#;
        let info = import_yay_from(bytes).expect("happy path yields Some");

        assert_eq!(info.info.len(), 2);

        let cava = info.info.get("cava-git").expect("cava-git present");
        assert_eq!(cava.repos.len(), 1);
        let repo = cava.repos.iter().next().expect("repo present");
        assert_eq!(repo.url, "https://github.com/karlstav/cava.git");
        assert_eq!(repo.branch, None);
        assert_eq!(repo.commit, "abc");

        let foo = info.info.get("foo-git").expect("foo-git present");
        let repo = find_repo(foo, "ssh://github.com/x/foo.git");
        assert_eq!(repo.branch.as_deref(), Some("dev"));
        assert_eq!(repo.commit, "def");
    }

    #[test]
    fn import_yay_from_normalizes_head_and_empty_branch() {
        let bytes = br#"{"pkg-git":{"example.com/repo.git":{"protocols":["https"],"branch":"HEAD","sha":"abc"},"example.com/other.git":{"protocols":["https"],"branch":"","sha":"def"}}}"#;
        let info = import_yay_from(bytes).expect("Some");

        let pkg = info.info.get("pkg-git").expect("pkg present");
        assert_eq!(pkg.repos.len(), 2);
        for repo in &pkg.repos {
            assert_eq!(repo.branch, None);
        }
    }

    #[test]
    fn import_yay_from_skips_repo_with_missing_or_empty_protocols() {
        let bytes = br#"{"pkg-git":{"example.com/kept.git":{"protocols":["https"],"sha":"abc"},"example.com/dropped.git":{"protocols":[],"sha":"def"},"example.com/alsodropped.git":{"sha":"ghi"}}}"#;
        let info = import_yay_from(bytes).expect("Some");

        let pkg = info.info.get("pkg-git").expect("pkg present");
        assert_eq!(pkg.repos.len(), 1);
        let repo = pkg.repos.iter().next().expect("repo present");
        assert_eq!(repo.url, "https://example.com/kept.git");
        assert_eq!(repo.commit, "abc");
    }

    #[test]
    fn import_yay_from_skips_repo_with_empty_sha() {
        let bytes = br#"{"pkg-git":{"example.com/kept.git":{"protocols":["https"],"sha":"abc"},"example.com/dropped.git":{"protocols":["https"],"sha":""}}}"#;
        let info = import_yay_from(bytes).expect("Some");

        let pkg = info.info.get("pkg-git").expect("pkg present");
        assert_eq!(pkg.repos.len(), 1);
        let repo = pkg.repos.iter().next().expect("repo present");
        assert_eq!(repo.url, "https://example.com/kept.git");
    }

    #[test]
    fn import_yay_from_corrupt_json_returns_none() {
        assert!(import_yay_from(b"{ not valid").is_none());
    }

    fn test_aur_info(name: &str, base: &str) -> AurInfo {
        AurInfo {
            id: 0,
            name: name.to_string(),
            package_base_id: 0,
            package_base: base.to_string(),
            version: "1.0".to_string(),
            description: None,
            url: None,
            num_votes: 0,
            popularity: 0.0,
            out_of_date: None,
            maintainer: None,
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: Vec::new(),
            make_depends: Vec::new(),
            check_depends: Vec::new(),
            opt_depends: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
            groups: Vec::new(),
            license: Vec::new(),
            keywords: Vec::new(),
            co_maintainers: Vec::new(),
        }
    }

    #[test]
    fn group_by_base_collapses_shared_base_in_order() {
        let infos = vec![
            test_aur_info("foo", "shared"),
            test_aur_info("bar", "shared"),
            test_aur_info("baz", "other"),
        ];
        let grouped = group_by_base(&infos);
        assert_eq!(grouped.len(), 2);
        assert_eq!(
            grouped.get("shared").expect("shared present"),
            &vec!["foo".to_string(), "bar".to_string()]
        );
        assert_eq!(
            grouped.get("other").expect("other present"),
            &vec!["baz".to_string()]
        );
    }
}
