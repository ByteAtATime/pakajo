use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};

use crate::aur::{AurClient, AurInfo};

#[derive(Debug)]
pub struct RaurError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl RaurError {
    pub fn new(message: String, source: Option<Box<dyn Error + Send + Sync>>) -> Self {
        Self { message, source }
    }

    pub fn from_anyhow(error: anyhow::Error) -> Self {
        let message = error.to_string();
        Self {
            message,
            source: Some(error.into_boxed_dyn_error()),
        }
    }
}

impl Display for RaurError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RaurError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|inner| inner as &(dyn Error + 'static))
    }
}

pub(crate) struct AurRaur(pub(crate) AurClient);

impl From<AurInfo> for raur::Package {
    fn from(info: AurInfo) -> Self {
        Self {
            id: info.id as u32,
            name: info.name,
            package_base_id: info.package_base_id as u32,
            package_base: info.package_base,
            version: info.version,
            description: info.description,
            url: info.url,
            num_votes: info.num_votes as u32,
            popularity: info.popularity,
            out_of_date: info.out_of_date,
            maintainer: info.maintainer,
            submitter: info.submitter,
            first_submitted: info.first_submitted,
            last_modified: info.last_modified,
            url_path: info.url_path.unwrap_or_default(),
            groups: info.groups,
            depends: info.depends,
            make_depends: info.make_depends,
            opt_depends: info.opt_depends,
            check_depends: info.check_depends,
            conflicts: info.conflicts,
            replaces: info.replaces,
            provides: info.provides,
            license: info.license,
            keywords: info.keywords,
            co_maintainers: info.co_maintainers,
        }
    }
}

#[async_trait::async_trait]
impl raur::Raur for AurRaur {
    type Err = RaurError;

    async fn raw_info<S: AsRef<str> + Send + Sync>(
        &self,
        pkgs: &[S],
    ) -> Result<Vec<raur::Package>, Self::Err> {
        let names: Vec<String> = pkgs.iter().map(|name| name.as_ref().to_string()).collect();
        self.0
            .info_many(&names)
            .map(|infos| infos.into_iter().map(Into::into).collect())
            .map_err(RaurError::from_anyhow)
    }

    async fn search_by<S: AsRef<str> + Send + Sync>(
        &self,
        query: S,
        by: raur::SearchBy,
    ) -> Result<Vec<raur::Package>, Self::Err> {
        self.0
            .search_by(query.as_ref(), &by.to_string())
            .map(|infos| infos.into_iter().map(Into::into).collect())
            .map_err(RaurError::from_anyhow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raur_error_preserves_source_chain() {
        let root = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
        let wrapped = anyhow::Error::new(root).context("AUR RPC request failed");
        let error = RaurError::from_anyhow(wrapped);
        assert_eq!(error.to_string(), "AUR RPC request failed");
        let first = error.source().expect("source is preserved");
        assert_eq!(first.to_string(), "AUR RPC request failed");
        let second = first.source().expect("chain continues");
        assert_eq!(second.to_string(), "connection refused");
    }
}
