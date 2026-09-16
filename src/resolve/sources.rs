use anyhow::Result;

use crate::aur::{AurClient, AurInfo};

use super::types::RepoPackage;

pub trait PackageDb {
    fn local_satisfier_exists(&self, dep: &str) -> bool;
    fn sync_satisfier(&self, dep: &str) -> Option<RepoPackage>;
}

pub struct AlpmDb<'a>(pub &'a alpm::Alpm);

impl<'a> PackageDb for AlpmDb<'a> {
    fn local_satisfier_exists(&self, dep: &str) -> bool {
        self.0.localdb().pkgs().find_satisfier(dep).is_some()
    }

    fn sync_satisfier(&self, dep: &str) -> Option<RepoPackage> {
        let pkg = self.0.syncdbs().find_satisfier(dep)?;
        Some(RepoPackage {
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
        })
    }
}

pub trait AurQuery {
    fn info_many(&self, names: &[String]) -> Result<Vec<AurInfo>>;
}

impl AurQuery for AurClient {
    fn info_many(&self, names: &[String]) -> Result<Vec<AurInfo>> {
        AurClient::info_many(self, names)
    }
}
