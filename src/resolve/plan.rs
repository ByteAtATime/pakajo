use serde::{Deserialize, Serialize};

use crate::aur::AurInfo;

use super::types::{BuildLayer, BuildPlan};

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
pub struct Unneeded {
    pub name: String,
    pub version: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenQuestion {
    Provider {
        depend: String,
        candidates: Vec<String>,
        chosen: String,
    },
    Group {
        group: String,
        members: Vec<GroupMember>,
        chosen: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub bases: Vec<Base>,
    pub repo_installs: Vec<RepoInstall>,
    pub missing: Vec<Missing>,
    pub unneeded: Vec<Unneeded>,
    pub conflicts: ConflictReport,
    pub duplicates: Vec<String>,
    pub questions: Vec<OpenQuestion>,
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

impl From<&aur_depends::Unneeded> for Unneeded {
    fn from(item: &aur_depends::Unneeded) -> Self {
        Self {
            name: item.name.clone(),
            version: item.version.clone(),
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

pub(crate) fn plan_from_actions(
    actions: &aur_depends::Actions<'_>,
    questions: Vec<OpenQuestion>,
) -> Plan {
    Plan {
        bases: actions.build.iter().map(Into::into).collect(),
        repo_installs: actions.install.iter().map(Into::into).collect(),
        missing: actions.missing.iter().map(Into::into).collect(),
        unneeded: actions.unneeded.iter().map(Into::into).collect(),
        conflicts: ConflictReport {
            local: conflicts(&actions.calculate_conflicts(true)),
            inner: conflicts(&actions.calculate_inner_conflicts(true)),
        },
        duplicates: actions.duplicate_targets(),
        questions,
    }
}

fn member_to_aur_info(base: &str, member: &Member) -> AurInfo {
    AurInfo {
        name: member.name.clone(),
        package_base: base.to_string(),
        version: member.version.clone(),
        ..Default::default()
    }
}

pub(crate) fn build_plan_from_plan(plan: &Plan, targets: &[String]) -> BuildPlan {
    let mut layers: Vec<BuildLayer> = (!plan.repo_installs.is_empty())
        .then(|| BuildLayer {
            aur: vec![],
            repo_deps: plan
                .repo_installs
                .iter()
                .map(|row| row.name.clone())
                .collect(),
        })
        .into_iter()
        .collect();
    layers.extend(plan.aur_builds().filter_map(|(name, members)| {
        members.first().map(|member| BuildLayer {
            aur: vec![member_to_aur_info(name, member)],
            repo_deps: vec![],
        })
    }));
    BuildPlan {
        targets: targets.to_vec(),
        layers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, version: &str, make: bool, target: bool) -> Member {
        Member {
            name: name.to_string(),
            version: version.to_string(),
            make,
            target,
        }
    }

    fn sample_plan() -> Plan {
        Plan {
            bases: vec![
                Base::Aur {
                    base: "helper".to_string(),
                    build: true,
                    members: vec![member("helper", "2.0-1", false, false)],
                },
                Base::Aur {
                    base: "top".to_string(),
                    build: true,
                    members: vec![member("top", "1.0-1", false, true)],
                },
            ],
            repo_installs: vec![RepoInstall {
                name: "glibc".to_string(),
                version: "2.39-1".to_string(),
                db: "core".to_string(),
                make: false,
                target: false,
            }],
            missing: vec![],
            unneeded: vec![],
            conflicts: ConflictReport {
                local: vec![],
                inner: vec![],
            },
            duplicates: vec![],
            questions: vec![],
        }
    }

    #[test]
    fn conversion_orders_repo_installs_before_bases() {
        let layers = build_plan_from_plan(&sample_plan(), &["top".to_string()]).layers;
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].repo_deps, vec!["glibc".to_string()]);
        assert!(layers[0].aur.is_empty());
        assert_eq!(layers[1].aur[0].name, "helper");
        assert_eq!(layers[1].aur[0].package_base, "helper");
        assert_eq!(layers[2].aur[0].name, "top");
        assert_eq!(layers[2].aur[0].version, "1.0-1");
    }
}
