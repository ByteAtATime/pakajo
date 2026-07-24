use std::collections::HashSet;

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Default)]
pub(crate) struct PkgInfo {
    pub repos: HashSet<RepoInfo>,
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
    use super::parse_url;

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
}
