use std::collections::HashSet;

use index::PackageIndex;

pub mod engine;
pub mod fuzzy;
pub mod index;
pub mod query;
pub mod tiers;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchFilter {
    #[default]
    All,
    Official,
    Aur,
    Installed,
}

impl SearchFilter {
    pub fn row_matches(self, index: &PackageIndex, pi: usize, installed: &HashSet<String>) -> bool {
        match self {
            SearchFilter::All => true,
            SearchFilter::Official => index.row(pi).is_repo,
            SearchFilter::Aur => !index.row(pi).is_repo,
            SearchFilter::Installed => installed.contains(index.name(pi)),
        }
    }
}
