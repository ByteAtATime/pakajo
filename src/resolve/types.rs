use crate::aur::AurInfo;

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
