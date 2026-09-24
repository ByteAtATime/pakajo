use crate::aur::AurInfo;
use alpm_utils::DbListExt;

#[derive(Debug, Clone)]
pub struct OptDependency {
    pub name: String,
    pub version: Option<String>,
    pub reason: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSource {
    Repo,
    Aur,
    Group,
}

#[derive(Debug, Clone)]
pub struct InstalledData {
    pub version: String,
    pub explicit: bool,
    pub install_date: Option<i64>,
    pub script: bool,
    pub installed_size: i64,
}

#[derive(Clone)]
pub struct Package {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub maintainer: Option<String>,
    pub licenses: Vec<String>,
    pub groups: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub dependencies: Vec<String>,
    pub opt_dependencies: Vec<OptDependency>,
    pub upstream_url: Option<String>,
    pub installed: Option<InstalledData>,
    pub kind: PackageKind,
}

#[derive(Debug, Clone)]
pub enum PackageKind {
    Repo(RepoData),
    Aur(AurData),
}

#[derive(Debug, Clone)]
pub struct RepoData {
    pub repo: Option<String>,
    pub architecture: Option<String>,
    pub installed_size: i64,
    pub download_size: i64,
    pub build_date: Option<i64>,
    pub validated_by: String,
    pub script: bool,
    pub required_by: Vec<String>,
    pub optional_for: Vec<String>,
}

impl RepoData {
    pub fn is_local(&self) -> bool {
        self.repo.as_deref() == Some("local")
    }
}

#[derive(Debug, Clone)]
pub struct AurData {
    pub num_votes: u64,
    pub popularity: f64,
    pub submitted: Option<i64>,
    pub last_modified: Option<i64>,
    pub flagged: Option<i64>,
    pub make_depends: Vec<String>,
    pub check_depends: Vec<String>,
}

fn normalized_epoch(value: i64) -> Option<i64> {
    (value > 0).then_some(value)
}

impl Package {
    pub fn source(&self) -> PackageSource {
        match &self.kind {
            PackageKind::Repo(_) => PackageSource::Repo,
            PackageKind::Aur(_) => PackageSource::Aur,
        }
    }

    pub fn repo(&self) -> Option<&str> {
        match &self.kind {
            PackageKind::Repo(data) => data.repo.as_deref(),
            PackageKind::Aur(_) => Some("aur"),
        }
    }

    pub fn maintainer_name(&self) -> Option<String> {
        let maintainer = self.maintainer.clone()?;
        if let Some((name, _rest)) = maintainer.split_once(" <") {
            Some(name.to_string())
        } else {
            Some(maintainer)
        }
    }
}

fn split_opt_constraint(raw: &str) -> (&str, Option<String>) {
    let Some(pos) = raw.find(['=', '>', '<']) else {
        return (raw, None);
    };
    if pos == 0 {
        return (raw, None);
    }
    let (name, version) = raw.split_at(pos);
    (name, Some(version.to_string()))
}

fn parse_opt_dependency(dependency: &str) -> Option<OptDependency> {
    if let Some((name, reason)) = dependency.split_once(": ") {
        let (bare, version) = split_opt_constraint(name);
        if bare.is_empty() {
            return None;
        }

        Some(OptDependency {
            name: bare.to_string(),
            version,
            reason: (!reason.is_empty()).then(|| reason.to_string()),
            installed: false,
        })
    } else {
        let (bare, version) = split_opt_constraint(dependency);
        if bare.is_empty() {
            return None;
        }

        Some(OptDependency {
            name: bare.to_string(),
            version,
            reason: None,
            installed: false,
        })
    }
}

fn validated_by_label(validation: alpm::PackageValidation) -> String {
    if validation.is_empty() {
        return "Unknown".to_string();
    }
    if validation.contains(alpm::PackageValidation::NONE) {
        return "None".to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    if validation.contains(alpm::PackageValidation::MD5SUM) {
        parts.push("MD5");
    }
    if validation.contains(alpm::PackageValidation::SHA256SUM) {
        parts.push("SHA-256");
    }
    if validation.contains(alpm::PackageValidation::SIGNATURE) {
        parts.push("Signature");
    }
    if parts.is_empty() {
        return "Unknown".to_string();
    }
    parts.join(", ")
}

impl From<&alpm::Package> for Package {
    fn from(pkg: &alpm::Package) -> Self {
        let build_date = pkg.build_date();
        Self {
            name: pkg.name().to_string(),
            description: pkg.desc().map(|x| x.to_string()),
            version: pkg.version().to_string(),
            maintainer: pkg.packager().map(|x| x.to_string()),
            licenses: pkg.licenses().iter().map(|x| x.to_string()).collect(),
            groups: pkg.groups().iter().map(|x| x.to_string()).collect(),
            provides: pkg.provides().iter().map(|x| x.to_string()).collect(),
            conflicts: pkg.conflicts().iter().map(|x| x.to_string()).collect(),
            replaces: pkg
                .replaces()
                .iter()
                .map(|x| x.name().to_string())
                .collect(),
            dependencies: pkg.depends().iter().map(|x| x.name().to_string()).collect(),
            opt_dependencies: pkg
                .optdepends()
                .iter()
                .map(|x| {
                    let version = match x.depmodver() {
                        alpm::DepModVer::Any => None,
                        alpm::DepModVer::Eq(v) => Some(format!("={v}")),
                        alpm::DepModVer::Ge(v) => Some(format!(">={v}")),
                        alpm::DepModVer::Le(v) => Some(format!("<={v}")),
                        alpm::DepModVer::Gt(v) => Some(format!(">{v}")),
                        alpm::DepModVer::Lt(v) => Some(format!("<{v}")),
                    };
                    OptDependency {
                        name: x.name().to_string(),
                        version,
                        reason: x.desc().map(|x| x.to_string()).filter(|d| !d.is_empty()),
                        installed: false,
                    }
                })
                .collect(),
            upstream_url: pkg.url().map(|x| x.to_string()),
            installed: None,
            kind: PackageKind::Repo(RepoData {
                repo: pkg.db().map(|x| x.name().to_string()),
                architecture: pkg.arch().map(|x| x.to_string()),
                installed_size: pkg.isize(),
                download_size: pkg.size(),
                build_date: (build_date > 0).then_some(build_date),
                validated_by: validated_by_label(pkg.validation()),
                script: pkg.has_scriptlet(),
                required_by: pkg.required_by().iter().map(|x| x.to_string()).collect(),
                optional_for: pkg.optional_for().iter().map(|x| x.to_string()).collect(),
            }),
        }
    }
}

impl From<AurInfo> for Package {
    fn from(info: AurInfo) -> Self {
        Self {
            name: info.name,
            description: info.description,
            version: info.version,
            maintainer: info.maintainer,
            licenses: info.license,
            groups: info.groups,
            provides: info.provides,
            conflicts: info.conflicts,
            replaces: info.replaces,
            dependencies: info.depends,
            opt_dependencies: info
                .opt_depends
                .iter()
                .filter_map(|x| parse_opt_dependency(&x.to_string()))
                .collect(),
            upstream_url: info.url,
            installed: None,
            kind: PackageKind::Aur(AurData {
                num_votes: info.num_votes,
                popularity: info.popularity,
                submitted: normalized_epoch(info.first_submitted),
                last_modified: normalized_epoch(info.last_modified),
                flagged: info.out_of_date.and_then(normalized_epoch),
                make_depends: info.make_depends,
                check_depends: info.check_depends,
            }),
        }
    }
}

pub fn opt_dep_installed(handle: &alpm::Alpm, dep: &OptDependency) -> bool {
    handle
        .localdb()
        .pkgs()
        .find_satisfier(format!(
            "{}{}",
            dep.name,
            dep.version.as_deref().unwrap_or("")
        ))
        .is_some()
}

pub fn is_installed(handle: &alpm::Alpm, name: &str) -> bool {
    handle.localdb().pkg(name).is_ok()
}

pub fn installed_names(handle: &alpm::Alpm) -> std::collections::HashSet<String> {
    handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .collect()
}

pub fn find(handle: &alpm::Alpm, name: &str) -> Option<Package> {
    handle.syncdbs().pkg(name).ok().map(Package::from)
}

#[derive(Debug, Clone)]
pub struct GroupMember {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PackageGroup {
    pub repo: String,
    pub members: Vec<GroupMember>,
}

pub fn find_groups(handle: &alpm::Alpm, name: &str) -> Vec<PackageGroup> {
    handle
        .syncdbs()
        .iter()
        .filter_map(|db| {
            db.group(name).ok().map(|group| PackageGroup {
                repo: db.name().to_string(),
                members: group
                    .packages()
                    .iter()
                    .map(|p| GroupMember {
                        name: p.name().to_string(),
                        description: p.desc().map(|d| d.to_string()),
                    })
                    .collect(),
            })
        })
        .collect()
}

pub fn local_group(handle: &alpm::Alpm, name: &str) -> Option<PackageGroup> {
    let db = handle.localdb();
    db.group(name).ok().map(|group| PackageGroup {
        repo: db.name().to_string(),
        members: group
            .packages()
            .iter()
            .map(|p| GroupMember {
                name: p.name().to_string(),
                description: p.desc().map(|d| d.to_string()),
            })
            .collect(),
    })
}

pub fn group_index(handle: &alpm::Alpm) -> Vec<(String, String)> {
    handle
        .syncdbs()
        .iter()
        .flat_map(|db| {
            db.groups()
                .into_iter()
                .flatten()
                .map(|g| (g.name().to_string(), db.name().to_string()))
        })
        .collect()
}

pub fn foreign_names(handle: &alpm::Alpm) -> Vec<String> {
    let sync_names: std::collections::HashSet<String> = handle
        .syncdbs()
        .iter()
        .flat_map(|db| db.pkgs().iter())
        .map(|p| p.name().to_string())
        .collect();
    handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .filter(|name| !sync_names.contains(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aur::AurInfo;

    #[test]
    fn parse_opt_dependency_cases() {
        let dep = parse_opt_dependency("foo-bar2").expect("foo-bar2 must parse");
        assert_eq!(dep.name, "foo-bar2");
        assert_eq!(dep.version, None);
        assert_eq!(dep.reason, None);

        let dep = parse_opt_dependency("foo>=1:2.0-1").expect("epoch constraint must parse");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.version.as_deref(), Some(">=1:2.0-1"));
        assert_eq!(dep.reason, None);

        let dep = parse_opt_dependency("foo=1.2").expect("equality constraint must parse");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.version.as_deref(), Some("=1.2"));
        assert_eq!(dep.reason, None);

        let dep = parse_opt_dependency("foo>=1.2: some reason").expect("reason split must parse");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.version.as_deref(), Some(">=1.2"));
        assert_eq!(dep.reason.as_deref(), Some("some reason"));

        let dep = parse_opt_dependency("foo: reason").expect("reason must parse");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.version, None);
        assert_eq!(dep.reason.as_deref(), Some("reason"));

        let dep = parse_opt_dependency("foo>=1:2.0-1: some reason")
            .expect("epoch with reason must parse");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.version.as_deref(), Some(">=1:2.0-1"));
        assert_eq!(dep.reason.as_deref(), Some("some reason"));
    }

    #[test]
    fn validated_by_label_cases() {
        use alpm::PackageValidation as V;
        assert_eq!(validated_by_label(V::empty()), "Unknown");
        assert_eq!(validated_by_label(V::NONE), "None");
        assert_eq!(validated_by_label(V::NONE | V::SIGNATURE), "None");
        assert_eq!(validated_by_label(V::MD5SUM), "MD5");
        assert_eq!(
            validated_by_label(V::SHA256SUM | V::SIGNATURE),
            "SHA-256, Signature"
        );
    }

    #[test]
    fn from_aur_info() {
        let info = AurInfo {
            id: 1,
            name: "foo".into(),
            package_base_id: 2,
            package_base: "foo".into(),
            version: "1.0-1".into(),
            description: Some("a pkg".into()),
            url: Some("https://example.com".into()),
            num_votes: 10,
            popularity: 5.0,
            out_of_date: None,
            maintainer: Some("maint".into()),
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: vec!["libc".into()],
            make_depends: vec!["gcc".into()],
            check_depends: vec!["bash".into()],
            opt_depends: vec!["foo-utils: extra tools".into()],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
            groups: vec![],
            license: vec!["MIT".into()],
            keywords: vec![],
            co_maintainers: vec![],
        };

        let pkg = Package::from(info);

        assert!(matches!(pkg.source(), PackageSource::Aur));
        assert_eq!(pkg.repo(), Some("aur"));
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.version, "1.0-1");
        let PackageKind::Aur(data) = &pkg.kind else {
            panic!("expected aur package kind");
        };
        assert_eq!(data.num_votes, 10);
        assert_eq!(data.popularity, 5.0);
        assert_eq!(pkg.upstream_url.as_deref(), Some("https://example.com"));
        assert_eq!(pkg.dependencies, vec!["libc"]);
        assert_eq!(pkg.licenses, vec!["MIT"]);
        assert_eq!(pkg.groups, Vec::<String>::new());
        assert_eq!(pkg.replaces, Vec::<String>::new());
        assert_eq!(pkg.opt_dependencies.len(), 1);
        assert_eq!(pkg.opt_dependencies[0].name, "foo-utils");
        assert_eq!(
            pkg.opt_dependencies[0].reason.as_deref(),
            Some("extra tools")
        );
    }

    #[test]
    fn find_groups_resolves_base_devel() {
        use crate::install::{offline_pkg, offline_root};
        let make = crate::install::OfflinePkg {
            name: "make",
            groups: &["base-devel"],
            ..offline_pkg("make")
        };
        let (_dir, handle) = offline_root(&[make]);
        let groups = find_groups(&handle, "base-devel");
        assert!(!groups.is_empty(), "base-devel group must resolve");
        let members: Vec<String> = groups
            .iter()
            .flat_map(|g| g.members.iter())
            .map(|m| m.name.clone())
            .collect();
        assert!(
            members.iter().any(|m| m == "make"),
            "base-devel should contain make; got {members:?}"
        );
    }

    #[test]
    fn group_index_includes_base_devel() {
        use crate::install::{offline_pkg, offline_root};
        let make = crate::install::OfflinePkg {
            name: "make",
            groups: &["base-devel"],
            ..offline_pkg("make")
        };
        let (_dir, handle) = offline_root(&[make]);
        let index = group_index(&handle);
        assert!(
            index.iter().any(|(name, _)| name == "base-devel"),
            "group index must contain base-devel; got {index:?}"
        );
    }

    #[test]
    fn local_group_lists_installed_members() {
        use crate::install::{drive_sync, offline_pkg, offline_root};
        use crate::question::source::ExploreDefaults;
        let make = crate::install::OfflinePkg {
            name: "make",
            groups: &["base-devel"],
            ..offline_pkg("make")
        };
        let (_dir, mut handle) = offline_root(&[make]);
        assert!(
            local_group(&handle, "base-devel").is_none(),
            "local base-devel must be absent before any member is installed"
        );
        drive_sync(
            &mut handle,
            &["make"],
            crate::tx::prompt::with_preapproved_proceed(Box::new(ExploreDefaults)),
        )
        .expect("make should install first");
        let group = local_group(&handle, "base-devel")
            .expect("base-devel should resolve in localdb after installing make");
        let members: Vec<String> = group.members.iter().map(|m| m.name.clone()).collect();
        assert!(
            members.iter().any(|m| m == "make"),
            "local base-devel should contain make; got {members:?}"
        );
    }
}
