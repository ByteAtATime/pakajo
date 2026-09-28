use std::collections::HashSet;

use index::PackageIndex;

pub mod engine;
pub mod fuzzy;
pub mod index;
pub mod query;
pub(crate) mod tiers;

pub use index::IndexRow;

#[derive(thiserror::Error, Debug)]
pub enum SearchError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode: {0}")]
    Decode(String),
    #[error("encode: {0}")]
    Encode(String),
    #[error("corrupt search cache")]
    Corrupt,
    #[error("store: {0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Repo,
    Aur,
    Group,
}

#[derive(Clone, Debug)]
pub struct PackageMeta {
    pub name: String,
    pub description: Option<String>,
    pub source: Source,
    pub repo: Option<String>,
    pub version: Option<String>,
    pub last_update: i64,
    pub num_votes: i64,
    pub popularity: f64,
}

#[derive(Clone, Debug)]
pub struct PackageGroup {
    pub name: String,
    pub repo: String,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub name: String,
    pub source: Source,
    pub description: Option<String>,
    pub version: Option<String>,
    pub repo: Option<String>,
    pub installed: bool,
    pub num_votes: i64,
    pub popularity: f64,
    pub last_update: i64,
}
