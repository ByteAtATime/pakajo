use crate::aur::AurInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Aur,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Explicit,
    Dep,
    MakeDep,
    CheckDep,
}

impl Reason {
    pub fn precedence(&self) -> u8 {
        match self {
            Reason::Explicit => 0,
            Reason::Dep => 1,
            Reason::MakeDep => 2,
            Reason::CheckDep => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct InstallNode {
    pub(super) name: String,
    pub(super) source: Source,
    pub(super) reason: Reason,
    pub(super) version: String,
    pub(super) aur_info: Option<AurInfo>,
}

#[derive(Debug, Clone)]
pub struct BuildLayer {
    pub aur: Vec<AurInfo>,
    pub repo_deps: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildPlan {
    pub targets: Vec<String>,
    pub layers: Vec<BuildLayer>,
}

#[derive(Debug, Clone)]
pub struct RepoPackage {
    pub name: String,
    pub version: String,
}
