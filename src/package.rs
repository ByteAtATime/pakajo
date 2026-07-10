use regex::Regex;

#[derive(Debug)]
pub struct OptDependency {
    pub name: String,
    pub reason: Option<String>,
}

pub struct Package {
    pub name: String,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub maintainer: Option<String>,
    pub licenses: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub dependencies: Vec<String>,
    pub opt_dependencies: Vec<OptDependency>,
    pub architecture: Option<String>,
    pub installed_size: i64,
    pub download_size: i64,
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
            name: pkg.name().to_string(),
            repo: pkg.db().map(|x| x.name().to_string()),
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
            architecture: pkg.arch().map(|x| x.to_string()),
            installed_size: pkg.isize(),
            download_size: pkg.size(),
        }
    }
}
