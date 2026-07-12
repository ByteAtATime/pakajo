use crate::aur::AurInfo;
use regex::Regex;

#[derive(Debug)]
pub struct OptDependency {
    pub name: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSource {
    Repo,
    #[allow(dead_code)]
    Aur,
}

#[allow(dead_code)]
pub struct Package {
    pub source: PackageSource,
    pub name: String,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub maintainer: Option<String>,
    pub licenses: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub dependencies: Vec<String>,
    pub make_dependencies: Vec<String>,
    pub check_dependencies: Vec<String>,
    pub opt_dependencies: Vec<OptDependency>,
    pub architecture: Option<String>,
    pub installed_size: Option<i64>,
    pub download_size: Option<i64>,
    pub num_votes: Option<u64>,
    pub popularity: Option<f64>,
    pub out_of_date: Option<i64>,
    pub upstream_url: Option<String>,
    pub package_base: Option<String>,
}

impl Package {
    pub fn maintainer_name(&self) -> Option<String> {
        let re = Regex::new(r"(.+) <.+>").expect("Failed to parse maintainer name regex");

        if let Some(groups) = re.captures(&self.maintainer.clone()?) {
            Some(groups[1].to_string())
        } else {
            self.maintainer.clone()
        }
    }
}

fn parse_opt_dependency(dependency: &str) -> Option<OptDependency> {
    // TODO: write a proper regex for package names + comparisons
    let re = Regex::new(r"([^:]+)(: (.+))?").expect("Failed to parse opt dependency regex");

    if let Some(groups) = re.captures(dependency) {
        Some(OptDependency {
            name: groups[1].to_string(),
            reason: groups.get(3).map(|x| x.as_str().to_string()), // TODO: better way to do this?
        })
    } else {
        None
    }
}

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
        Self {
            source: PackageSource::Repo,
            name: pkg.name().to_string(),
            repo: pkg.db().map(|x| x.name().to_string()),
            description: pkg.desc().map(|x| x.to_string()),
            version: pkg.version().to_string(),
            maintainer: pkg.packager().map(|x| x.to_string()),
            licenses: pkg.licenses().iter().map(|x| x.to_string()).collect(),
            provides: pkg.provides().iter().map(|x| x.to_string()).collect(),
            conflicts: pkg.conflicts().iter().map(|x| x.to_string()).collect(),
            dependencies: pkg.depends().iter().map(|x| x.to_string()).collect(),
            make_dependencies: Vec::new(),
            check_dependencies: Vec::new(),
            opt_dependencies: pkg
                .optdepends()
                .iter()
                .filter_map(|x| parse_opt_dependency(&x.to_string()))
                .collect(),
            architecture: pkg.arch().map(|x| x.to_string()),
            installed_size: Some(pkg.isize()),
            download_size: Some(pkg.size()),
            num_votes: None,
            popularity: None,
            out_of_date: None,
            upstream_url: None,
            package_base: None,
        }
    }
}

impl From<AurInfo> for Package {
    fn from(info: AurInfo) -> Self {
        Self {
            source: PackageSource::Aur,
            name: info.name,
            repo: Some("aur".to_string()),
            description: info.description,
            version: info.version,
            maintainer: info.maintainer,
            licenses: info.license,
            provides: info.provides,
            conflicts: info.conflicts,
            dependencies: info.depends,
            make_dependencies: info.make_depends,
            check_dependencies: info.check_depends,
            opt_dependencies: info
                .opt_depends
                .iter()
                .filter_map(|x| parse_opt_dependency(&x.to_string()))
                .collect(),
            architecture: None,
            installed_size: None,
            download_size: None,
            num_votes: Some(info.num_votes),
            popularity: Some(info.popularity),
            out_of_date: info.out_of_date,
            upstream_url: info.url,
            package_base: Some(info.package_base),
        }
    }
}

pub fn is_installed(handle: &alpm::Alpm, name: &str) -> bool {
    handle.localdb().pkg(name).is_ok()
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

        assert_eq!(pkg.source, PackageSource::Aur);
        assert_eq!(pkg.repo.as_deref(), Some("aur"));
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, "1.0-1");
        assert_eq!(pkg.installed_size, None);
        assert_eq!(pkg.download_size, None);
        assert_eq!(pkg.num_votes, Some(10));
        assert_eq!(pkg.popularity, Some(5.0));
        assert_eq!(pkg.package_base.as_deref(), Some("foo"));
        assert_eq!(pkg.upstream_url.as_deref(), Some("https://example.com"));
        assert_eq!(pkg.dependencies, vec!["libc"]);
        assert_eq!(pkg.make_dependencies, vec!["gcc"]);
        assert_eq!(pkg.check_dependencies, vec!["bash"]);
        assert_eq!(pkg.licenses, vec!["MIT"]);
        assert_eq!(pkg.opt_dependencies.len(), 1);
        assert_eq!(pkg.opt_dependencies[0].name, "foo-utils");
        assert_eq!(
            pkg.opt_dependencies[0].reason.as_deref(),
            Some("extra tools")
        );
    }
}
