pub struct Package {
    pub name: String,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub maintainer: Option<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
}

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
        Self {
            name: pkg.name().to_string(),
            repo: pkg.db().map(|x| x.name().to_string()),
            description: pkg.desc().map(|x| x.to_string()),
            version: pkg.version().to_string(),
            maintainer: pkg.packager().map(|x| x.to_string()),
            provides: pkg.provides().iter().map(|x| x.to_string()).collect(),
            conflicts: pkg.conflicts().iter().map(|x| x.to_string()).collect(),
        }
    }
}
