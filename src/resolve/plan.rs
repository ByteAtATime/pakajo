use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub version: String,
    pub make: bool,
    pub target: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Base {
    Aur {
        base: String,
        build: bool,
        members: Vec<Member>,
    },
    Pkgbuild {
        repo: String,
        base: String,
        build: bool,
        members: Vec<Member>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInstall {
    pub name: String,
    pub version: String,
    pub db: String,
    pub make: bool,
    pub target: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingStack {
    pub pkg: String,
    pub dep: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missing {
    pub dep: String,
    pub stack: Vec<MissingStack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflicting {
    pub pkg: String,
    pub conflict: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub pkg: String,
    pub conflicting: Vec<Conflicting>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub local: Vec<Conflict>,
    pub inner: Vec<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMember {
    pub name: String,
    pub version: String,
    pub db: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub bases: Vec<Base>,
    pub repo_installs: Vec<RepoInstall>,
    pub missing: Vec<Missing>,
    pub conflicts: ConflictReport,
    pub duplicates: Vec<String>,
}

impl ConflictReport {
    pub fn is_empty(&self) -> bool {
        self.local.is_empty() && self.inner.is_empty()
    }
}

impl Base {
    pub fn members(&self) -> &[Member] {
        match self {
            Base::Aur { members, .. } => members,
            Base::Pkgbuild { members, .. } => members,
        }
    }

    pub fn aur_build(&self) -> Option<(&str, &[Member])> {
        match self {
            Base::Aur {
                base,
                build: true,
                members,
            } => Some((base.as_str(), members.as_slice())),
            _ => None,
        }
    }
}

impl Plan {
    pub fn all_members(&self) -> impl Iterator<Item = &Member> {
        self.bases.iter().flat_map(|base| base.members())
    }

    pub fn aur_builds(&self) -> impl Iterator<Item = (&str, &[Member])> {
        self.bases.iter().filter_map(|base| base.aur_build())
    }
}

impl From<&aur_depends::Conflict> for Conflict {
    fn from(conflict: &aur_depends::Conflict) -> Self {
        Self {
            pkg: conflict.pkg.clone(),
            conflicting: conflict.conflicting.iter().map(Into::into).collect(),
        }
    }
}

impl From<&aur_depends::Conflicting> for Conflicting {
    fn from(other: &aur_depends::Conflicting) -> Self {
        Self {
            pkg: other.pkg.clone(),
            conflict: other.conflict.clone(),
        }
    }
}

impl From<&aur_depends::Missing> for Missing {
    fn from(item: &aur_depends::Missing) -> Self {
        Self {
            dep: item.dep.clone(),
            stack: item.stack.iter().map(Into::into).collect(),
        }
    }
}

impl From<&aur_depends::DepMissing> for MissingStack {
    fn from(frame: &aur_depends::DepMissing) -> Self {
        Self {
            pkg: frame.pkg.clone(),
            dep: frame.dep.clone(),
        }
    }
}

impl From<&aur_depends::RepoPackage<'_>> for RepoInstall {
    fn from(row: &aur_depends::RepoPackage<'_>) -> Self {
        Self {
            name: row.pkg.name().to_string(),
            version: row.pkg.version().to_string(),
            db: row
                .pkg
                .db()
                .map(|db| db.name().to_string())
                .unwrap_or_default(),
            make: row.make,
            target: row.target,
        }
    }
}

impl From<&aur_depends::Base> for Base {
    fn from(base: &aur_depends::Base) -> Self {
        match base {
            aur_depends::Base::Aur(found) => Base::Aur {
                base: found.package_base().to_string(),
                build: found.build,
                members: found
                    .pkgs
                    .iter()
                    .map(|member| Member {
                        name: member.pkg.name.clone(),
                        version: member.pkg.version.clone(),
                        make: member.make,
                        target: member.target,
                    })
                    .collect(),
            },
            aur_depends::Base::Pkgbuild(found) => Base::Pkgbuild {
                repo: found.repo.clone(),
                base: found.package_base().to_string(),
                build: found.build,
                members: found
                    .pkgs
                    .iter()
                    .map(|member| Member {
                        name: member.pkg.pkgname.clone(),
                        version: found.srcinfo.version(),
                        make: member.make,
                        target: member.target,
                    })
                    .collect(),
            },
        }
    }
}

fn conflicts(items: &[aur_depends::Conflict]) -> Vec<Conflict> {
    items.iter().map(Conflict::from).collect()
}

pub(crate) fn plan_from_actions(actions: &aur_depends::Actions<'_>) -> Plan {
    Plan {
        bases: actions.build.iter().map(Into::into).collect(),
        repo_installs: actions.install.iter().map(Into::into).collect(),
        missing: actions.missing.iter().map(Into::into).collect(),
        conflicts: ConflictReport {
            local: conflicts(&actions.calculate_conflicts(true)),
            inner: conflicts(&actions.calculate_inner_conflicts(true)),
        },
        duplicates: actions.duplicate_targets(),
    }
}
