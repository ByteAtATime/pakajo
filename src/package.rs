use crate::aur::AurInfo;

#[derive(Debug, Clone)]
pub struct OptDependency {
    pub name: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSource {
    Repo,
    Aur,
    Group,
}

#[derive(Clone)]
pub struct Package {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub maintainer: Option<String>,
    pub licenses: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub dependencies: Vec<String>,
    pub opt_dependencies: Vec<OptDependency>,
    pub upstream_url: Option<String>,
    pub kind: PackageKind,
}

#[derive(Debug, Clone)]
pub enum PackageKind {
    Repo(RepoData),
    Aur(AurData),
}

#[derive(Debug, Clone)]
pub struct RepoData {
    pub repo: Option<String>,
    pub architecture: Option<String>,
    pub installed_size: i64,
    pub download_size: i64,
}

#[derive(Debug, Clone)]
pub struct AurData {
    pub num_votes: u64,
    pub popularity: f64,
}

impl Package {
    pub fn source(&self) -> PackageSource {
        match &self.kind {
            PackageKind::Repo(_) => PackageSource::Repo,
            PackageKind::Aur(_) => PackageSource::Aur,
        }
    }

    pub fn repo(&self) -> Option<&str> {
        match &self.kind {
            PackageKind::Repo(data) => data.repo.as_deref(),
            PackageKind::Aur(_) => Some("aur"),
        }
    }

    pub fn maintainer_name(&self) -> Option<String> {
        let maintainer = self.maintainer.clone()?;
        if let Some((name, _rest)) = maintainer.split_once(" <") {
            Some(name.to_string())
        } else {
            Some(maintainer)
        }
    }
}

fn parse_opt_dependency(dependency: &str) -> Option<OptDependency> {
    if let Some((name, reason)) = dependency.split_once(": ") {
        if name.is_empty() {
            return None;
        }

        Some(OptDependency {
            name: name.to_string(),
            reason: (!reason.is_empty()).then(|| reason.to_string()),
        })
    } else {
        if dependency.is_empty() {
            return None;
        }

        Some(OptDependency {
            name: dependency.to_string(),
            reason: None,
        })
    }
}

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
        Self {
            name: pkg.name().to_string(),
            description: pkg.desc().map(|x| x.to_string()),
            version: pkg.version().to_string(),
            maintainer: pkg.packager().map(|x| x.to_string()),
            licenses: pkg.licenses().iter().map(|x| x.to_string()).collect(),
            provides: pkg.provides().iter().map(|x| x.to_string()).collect(),
            conflicts: pkg.conflicts().iter().map(|x| x.to_string()).collect(),
            dependencies: pkg.depends().iter().map(|x| x.to_string()).collect(),
            opt_dependencies: pkg
                .optdepends()
                .iter()
                .filter_map(|x| parse_opt_dependency(&x.to_string()))
                .collect(),
            upstream_url: pkg.url().map(|x| x.to_string()),
            kind: PackageKind::Repo(RepoData {
                repo: pkg.db().map(|x| x.name().to_string()),
                architecture: pkg.arch().map(|x| x.to_string()),
                installed_size: pkg.isize(),
                download_size: pkg.size(),
            }),
        }
    }
}

impl From<AurInfo> for Package {
    fn from(info: AurInfo) -> Self {
        Self {
            name: info.name,
            description: info.description,
            version: info.version,
            maintainer: info.maintainer,
            licenses: info.license,
            provides: info.provides,
            conflicts: info.conflicts,
            dependencies: info.depends,
            opt_dependencies: info
                .opt_depends
                .iter()
                .filter_map(|x| parse_opt_dependency(&x.to_string()))
                .collect(),
            upstream_url: info.url,
            kind: PackageKind::Aur(AurData {
                num_votes: info.num_votes,
                popularity: info.popularity,
            }),
        }
    }
}

pub fn is_installed(handle: &alpm::Alpm, name: &str) -> bool {
    handle.localdb().pkg(name).is_ok()
}

pub fn installed_names(handle: &alpm::Alpm) -> std::collections::HashSet<String> {
    handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aur::AurInfo;

    #[test]
    fn from_aur_info() {
        let info = AurInfo {
            id: 1,
            name: "foo".into(),
            package_base_id: 2,
            package_base: "foo".into(),
            version: "1.0-1".into(),
            description: Some("a pkg".into()),
            url: Some("https://example.com".into()),
            num_votes: 10,
            popularity: 5.0,
            out_of_date: None,
            maintainer: Some("maint".into()),
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: vec!["libc".into()],
            make_depends: vec!["gcc".into()],
            check_depends: vec!["bash".into()],
            opt_depends: vec!["foo-utils: extra tools".into()],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
            groups: vec![],
            license: vec!["MIT".into()],
            keywords: vec![],
            co_maintainers: vec![],
        };

        let pkg = Package::from(info);

        assert!(matches!(pkg.source(), PackageSource::Aur));
        assert_eq!(pkg.repo(), Some("aur"));
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, "1.0-1");
        let PackageKind::Aur(data) = &pkg.kind else {
            panic!("expected aur package kind");
        };
        assert_eq!(data.num_votes, 10);
        assert_eq!(data.popularity, 5.0);
        assert_eq!(pkg.upstream_url.as_deref(), Some("https://example.com"));
        assert_eq!(pkg.dependencies, vec!["libc"]);
        assert_eq!(pkg.licenses, vec!["MIT"]);
        assert_eq!(pkg.opt_dependencies.len(), 1);
        assert_eq!(pkg.opt_dependencies[0].name, "foo-utils");
        assert_eq!(
            pkg.opt_dependencies[0].reason.as_deref(),
            Some("extra tools")
        );
    }
}
