use regex::Regex;

#[derive(Debug)]
pub struct OptDependency {
    pub name: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSource {
    Repo,
    Aur,
}

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

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
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

pub fn is_installed(handle: &alpm::Alpm, name: &str) -> bool {
    handle.localdb().pkg(name).is_ok()
}
