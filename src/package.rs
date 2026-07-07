pub struct Package {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
}

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
        Self {
            name: pkg.name().to_string(),
            description: pkg.desc().map(|x| x.to_string()),
            version: pkg.version().to_string(),
        }
    }
}
