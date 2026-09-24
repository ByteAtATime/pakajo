use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use anyhow::Context as _;

use crate::aur::{AurClient, AurInfo};
use crate::events::{InstallEvent, InstallSink};

pub fn run_upgrade_repo(
    no_refresh: bool,
    extra_ignores: &[String],
    source: Box<dyn crate::question::source::AnswerSource>,
    sink: Box<dyn InstallSink>,
) -> anyhow::Result<crate::tx::driver::RunOutcome> {
    let config = crate::pacman::config()?;
    let mut handle = crate::pacman::handle_with_config(&config)?;
    apply_ignores(&mut handle, &config, extra_ignores);
    let events: Rc<RefCell<Box<dyn InstallSink>>> = Rc::new(RefCell::new(sink));
    if !no_refresh {
        crate::tx::driver::forward_alpm_events(&mut handle, &events);
        events.borrow_mut().event(InstallEvent::SyncDatabases);
        crate::pacman::lock::lock_retry(
            || handle.syncdbs_mut().update(false).map(|_| ()),
            || {
                events
                    .borrow_mut()
                    .event(InstallEvent::WaitingForDatabaseLock)
            },
            crate::pacman::lock::LOCK_POLL_INTERVAL,
        )
        .context("failed to refresh sync DBs")?;
    }
    let spec = crate::tx::driver::RunSpec {
        kind: crate::tx::driver::RunKind::Upgrade,
        targets: Vec::new(),
        stub_targets: Vec::new(),
        explore: false,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    };
    crate::tx::driver::run_with_events(&mut handle, &spec, source, &events)
}

pub fn apply_ignores(handle: &mut alpm::Alpm, config: &pacmanconf::Config, extra: &[String]) {
    for name in &config.ignore_pkg {
        if handle.add_ignorepkg(name.as_str()).is_err() {
            eprintln!("warning: failed to ignore package {name}");
        }
    }
    for group in &config.ignore_group {
        if handle.add_ignoregroup(group.as_str()).is_err() {
            eprintln!("warning: failed to ignore group {group}");
        }
    }
    for name in extra {
        if handle.add_ignorepkg(name.as_str()).is_err() {
            eprintln!("warning: failed to ignore package {name}");
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AurUpgradeCandidate {
    pub name: String,
    pub local_version: String,
    pub remote_version: String,
    pub package_base: String,
}

fn select_upgradable_candidates(
    installed: Vec<(String, String)>,
    sync_names: &HashSet<String>,
    aur_infos: &HashMap<String, AurInfo>,
) -> Vec<AurUpgradeCandidate> {
    let mut candidates: Vec<AurUpgradeCandidate> = installed
        .into_iter()
        .filter_map(|(name, local_version)| {
            if sync_names.contains(&name) {
                return None;
            }
            let info = aur_infos.get(&name)?;
            if alpm::vercmp(info.version.clone(), local_version.clone())
                != std::cmp::Ordering::Greater
            {
                return None;
            }
            Some(AurUpgradeCandidate {
                name,
                local_version,
                remote_version: info.version.clone(),
                package_base: info.package_base.clone(),
            })
        })
        .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevelSource {
    Live,
    Cached(Vec<String>),
}

pub fn compute_aur_upgrades(
    handle: &alpm::Alpm,
    aur: &AurClient,
    devel: DevelSource,
) -> anyhow::Result<(Vec<AurUpgradeCandidate>, Vec<String>)> {
    let sync_names: HashSet<String> = handle
        .syncdbs()
        .iter()
        .flat_map(|db| db.pkgs().iter())
        .map(|p| p.name().to_string())
        .collect();
    let installed: Vec<(String, String)> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| (p.name().to_string(), p.version().to_string()))
        .collect();
    let foreign_names: Vec<String> = crate::package::foreign_names(handle);
    if foreign_names.is_empty() {
        return Ok((vec![], vec![]));
    }
    let infos = aur.info_many(&foreign_names)?;
    let aur_infos: HashMap<String, AurInfo> =
        infos.into_iter().map(|i| (i.name.clone(), i)).collect();
    let mut candidates = select_upgradable_candidates(installed, &sync_names, &aur_infos);

    let devel_updates = match devel {
        DevelSource::Live => crate::devel::possible_devel_updates(),
        DevelSource::Cached(names) => names,
    };
    let mut devel_names: Vec<String> = devel_updates
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    devel_names.sort();
    merge_devel_candidates(&mut candidates, &devel_names, |name| {
        let pkg = handle.localdb().pkg(name).ok()?;
        Some((pkg.version().to_string(), pkg.base().map(str::to_string)))
    });

    Ok((candidates, devel_names))
}

fn merge_devel_candidates(
    candidates: &mut Vec<AurUpgradeCandidate>,
    devel_names: &[String],
    lookup: impl Fn(&str) -> Option<(String, Option<String>)>,
) {
    let existing: HashSet<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
    let stale_devel_names: Vec<String> = devel_names
        .iter()
        .filter(|name| !existing.contains(name.as_str()))
        .cloned()
        .collect();
    for name in &stale_devel_names {
        let Some((local_version, package_base)) = lookup(name) else {
            continue;
        };
        candidates.push(AurUpgradeCandidate {
            name: name.clone(),
            local_version,
            remote_version: "latest-commit".to_string(),
            package_base: package_base.unwrap_or_else(|| name.clone()),
        });
    }
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn select_upgradable_candidates_filters_and_compares() {
        let installed = vec![
            ("foo".to_string(), "1.0".to_string()),
            ("bar".to_string(), "1.0".to_string()),
            ("baz".to_string(), "1.0".to_string()),
            ("qux".to_string(), "1.0".to_string()),
            ("wal".to_string(), "2.0".to_string()),
        ];
        let sync_names = HashSet::from(["baz".to_string()]);
        let mut aur_infos = HashMap::new();
        aur_infos.insert("foo".to_string(), test_aur_info("foo", "1.1"));
        aur_infos.insert("bar".to_string(), test_aur_info("bar", "1.0"));
        aur_infos.insert("wal".to_string(), test_aur_info("wal", "1.5"));

        let result = select_upgradable_candidates(installed, &sync_names, &aur_infos);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "foo");
        assert_eq!(result[0].local_version, "1.0");
        assert_eq!(result[0].remote_version, "1.1");
        assert_eq!(result[0].package_base, "foo");
    }

    #[test]
    fn select_upgradable_candidates_sorted_deterministically() {
        let installed = vec![
            ("zeta".to_string(), "1.0".to_string()),
            ("alpha".to_string(), "1.0".to_string()),
            ("mid".to_string(), "1.0".to_string()),
        ];
        let sync_names = HashSet::new();
        let aur_infos = HashMap::from([
            ("zeta".to_string(), test_aur_info("zeta", "2.0")),
            ("alpha".to_string(), test_aur_info("alpha", "2.0")),
            ("mid".to_string(), test_aur_info("mid", "2.0")),
        ]);

        let result = select_upgradable_candidates(installed, &sync_names, &aur_infos);
        assert_eq!(
            result.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "mid", "zeta"]
        );
    }

    fn test_aur_info(name: &str, version: &str) -> AurInfo {
        AurInfo {
            id: 0,
            name: name.to_string(),
            package_base_id: 0,
            package_base: name.to_string(),
            version: version.to_string(),
            description: None,
            url: None,
            num_votes: 0,
            popularity: 0.0,
            out_of_date: None,
            maintainer: None,
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: Vec::new(),
            make_depends: Vec::new(),
            check_depends: Vec::new(),
            opt_depends: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
            groups: Vec::new(),
            license: Vec::new(),
            keywords: Vec::new(),
            co_maintainers: Vec::new(),
        }
    }

    fn candidate(name: &str) -> AurUpgradeCandidate {
        AurUpgradeCandidate {
            name: name.to_string(),
            local_version: "1.0".to_string(),
            remote_version: "2.0".to_string(),
            package_base: name.to_string(),
        }
    }

    #[test]
    fn merge_devel_candidates_skips_existing_names() {
        let mut candidates = vec![candidate("foo-git")];
        let devel_names = vec!["foo-git".to_string()];
        merge_devel_candidates(&mut candidates, &devel_names, |_| {
            Some(("1.5".to_string(), Some("foo-git".to_string())))
        });
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].remote_version, "2.0");
    }

    #[test]
    fn merge_devel_candidates_skips_missing_local_pkgs() {
        let mut candidates = vec![candidate("aaa")];
        let devel_names = vec!["gone-git".to_string()];
        merge_devel_candidates(&mut candidates, &devel_names, |_| None);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "aaa");
    }

    #[test]
    fn merge_devel_candidates_adds_latest_commit_with_base_fallback() {
        let mut candidates = Vec::new();
        let devel_names = vec!["baseless-git".to_string(), "based-git".to_string()];
        let mut bases = HashMap::new();
        bases.insert("baseless-git", None::<String>);
        bases.insert("based-git", Some("based".to_string()));
        merge_devel_candidates(&mut candidates, &devel_names, |name| {
            bases
                .get(name)
                .map(|base| ("r100.g0".to_string(), base.clone()))
        });
        assert_eq!(candidates.len(), 2);
        let by_name = |n: &str| candidates.iter().find(|c| c.name == n).unwrap();
        let baseless = by_name("baseless-git");
        assert_eq!(baseless.remote_version, "latest-commit");
        assert_eq!(baseless.local_version, "r100.g0");
        assert_eq!(baseless.package_base, "baseless-git");
        let based = by_name("based-git");
        assert_eq!(based.package_base, "based");
    }

    #[test]
    fn merge_devel_candidates_sorts_final_result_by_name() {
        let mut candidates = vec![candidate("zeta")];
        let devel_names = vec!["alpha-git".to_string(), "mid-git".to_string()];
        merge_devel_candidates(&mut candidates, &devel_names, |name| {
            Some((("1.0".to_string()), Some(name.to_string())))
        });
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha-git", "mid-git", "zeta"]
        );
    }

    #[test]
    #[ignore]
    fn compute_aur_upgrades_live() {
        let handle = crate::pacman::handle().expect("alpm");
        let aur = crate::aur::AurClient::new();
        let candidates = compute_aur_upgrades(&handle, &aur, DevelSource::Live)
            .expect("rpc succeeds")
            .0;
        println!("candidates: {candidates:?}");
    }

    #[test]
    fn apply_ignores_propagates_to_handle() {
        let base = std::env::temp_dir().join("pakajo_upgrade_ignores_test");
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let db = base.join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().into_owned(),
            db.to_string_lossy().into_owned(),
        )
        .unwrap();

        let mut config = pacmanconf::Config::default();
        config.ignore_pkg = vec!["foo".to_string()];
        config.ignore_group = vec!["bar".to_string()];

        apply_ignores(&mut handle, &config, &["baz".to_string()]);

        let pkgs: HashSet<String> = handle.ignorepkgs().iter().map(|s| s.to_string()).collect();
        let groups: HashSet<String> = handle
            .ignoregroups()
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert!(
            pkgs.contains("foo"),
            "ignore_pkg from config must be set: {pkgs:?}"
        );
        assert!(pkgs.contains("baz"), "extra ignore must be set: {pkgs:?}");
        assert!(
            groups.contains("bar"),
            "ignore_group from config must be set: {groups:?}"
        );
    }
}
