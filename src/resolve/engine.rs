use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;

use crate::aur::AurInfo;

use super::graph::DepGraph;
use super::satisfies::{pkg_satisfies, provide_satisfies, satisfies_aur, split_dep};
use super::sources::{AurQuery, PackageDb};
use super::types::{BuildLayer, BuildPlan, InstallNode, Reason, RepoPackage, Source};

fn set_reason_with_precedence(node: &mut InstallNode, reason: Reason) {
    if reason.precedence() < node.reason.precedence() {
        node.reason = reason;
    }
}

fn upsert_repo(nodes: &mut HashMap<String, InstallNode>, r: &RepoPackage, reason: Reason) {
    match nodes.get_mut(&r.name) {
        Some(node) => set_reason_with_precedence(node, reason),
        None => {
            nodes.insert(
                r.name.clone(),
                InstallNode {
                    name: r.name.clone(),
                    source: Source::Repo,
                    reason,
                    version: r.version.clone(),
                    aur_info: None,
                },
            );
        }
    }
}

fn upsert_aur(nodes: &mut HashMap<String, InstallNode>, pkg: AurInfo, reason: Reason) {
    match nodes.get_mut(&pkg.name) {
        Some(node) => set_reason_with_precedence(node, reason),
        None => {
            nodes.insert(
                pkg.name.clone(),
                InstallNode {
                    name: pkg.name.clone(),
                    source: Source::Aur,
                    reason,
                    version: pkg.version.clone(),
                    aur_info: Some(pkg),
                },
            );
        }
    }
}

pub fn resolve(
    db: &impl PackageDb,
    aur: &impl AurQuery,
    targets: &[String],
    no_check: bool,
) -> Result<BuildPlan> {
    if targets.is_empty() {
        return Ok(BuildPlan {
            targets: vec![],
            layers: vec![],
        });
    }

    let mut nodes: HashMap<String, InstallNode> = HashMap::new();
    let mut provider_cache: HashMap<String, Vec<AurInfo>> = HashMap::new();
    let mut seen_for_expansion: HashSet<String> = HashSet::new();
    let mut worklist: VecDeque<AurInfo> = VecDeque::new();
    let mut graph = DepGraph::new();

    let found = aur.info_many(targets)?;
    for target in targets {
        let Some(pkg) = found.iter().find(|p| &p.name == target).cloned() else {
            anyhow::bail!("target not found in AUR: {target}");
        };
        graph.add_node(&pkg.name);
        for provide in &pkg.provides {
            graph.add_provides(provide, &pkg.name, &pkg.version);
        }
        nodes.insert(
            pkg.name.clone(),
            InstallNode {
                name: pkg.name.clone(),
                source: Source::Aur,
                reason: Reason::Explicit,
                version: pkg.version.clone(),
                aur_info: Some(pkg.clone()),
            },
        );
        seen_for_expansion.insert(pkg.name.clone());
        worklist.push_back(pkg);
    }

    while let Some(pkg) = worklist.pop_front() {
        let parent = pkg.name.clone();
        let mut ordered: Vec<(String, Reason)> = Vec::new();
        for d in &pkg.make_depends {
            ordered.push((d.clone(), Reason::MakeDep));
        }
        for d in &pkg.depends {
            ordered.push((d.clone(), Reason::Dep));
        }
        if !no_check {
            for d in &pkg.check_depends {
                ordered.push((d.clone(), Reason::CheckDep));
            }
        }

        let mut aur_batch: Vec<(String, Reason)> = Vec::new();
        for (dep_string, reason) in ordered {
            let dep_name = split_dep(&dep_string).name.to_string();

            if graph.exists(&dep_name) {
                let matched = nodes.get(&dep_name).is_some_and(|n| {
                    pkg_satisfies(&n.name, &n.version, &dep_string)
                });
                if matched {
                    graph.depend_on(&parent, &dep_name);
                    if let Some(node) = nodes.get_mut(&dep_name) {
                        set_reason_with_precedence(node, reason);
                    }
                    continue;
                }
            }

            if db.local_satisfier_exists(&dep_string) {
                continue;
            }

            if let Some(r) = db.sync_satisfier(&dep_string) {
                graph.depend_on(&parent, &r.name);
                upsert_repo(&mut nodes, &r, reason);
                continue;
            }

            let provider_match = graph.has_provides(&dep_name).and_then(|entry| {
                if provide_satisfies(&entry.provide, &dep_string, &entry.provider_version) {
                    Some(entry.provider.clone())
                } else {
                    None
                }
            });
            if let Some(provider) = provider_match {
                graph.depend_on(&parent, &provider);
                if let Some(node) = nodes.get_mut(&provider) {
                    set_reason_with_precedence(node, reason);
                }
                continue;
            }

            aur_batch.push((dep_string, reason));
        }

        if !aur_batch.is_empty() {
            let mut uncached: Vec<String> = {
                let mut set: HashSet<String> = HashSet::new();
                for (d, _) in &aur_batch {
                    let n = split_dep(d).name.to_string();
                    if !provider_cache.contains_key(&n) {
                        set.insert(n);
                    }
                }
                let mut v: Vec<String> = set.into_iter().collect();
                v.sort();
                v
            };
            if !uncached.is_empty() {
                let results = aur.info_many(&uncached)?;
                for p in results {
                    let name = p.name.clone();
                    let provides = p.provides.clone();
                    for provide in &provides {
                        let pname = split_dep(provide).name.to_string();
                        provider_cache.entry(pname).or_default().push(p.clone());
                    }
                    provider_cache.entry(name).or_default().push(p);
                }
            }
            uncached.clear();

            for (dep_string, reason) in aur_batch {
                let dep_name = split_dep(&dep_string).name.to_string();
                let mut candidates: Vec<AurInfo> = provider_cache
                    .get(&dep_name)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| satisfies_aur(&dep_string, p))
                    .collect();
                if candidates.is_empty() {
                    let filtered: Vec<AurInfo> = aur
                        .search_by_provides(&dep_name)?
                        .into_iter()
                        .filter(|p| satisfies_aur(&dep_string, p))
                        .collect();
                    provider_cache.insert(dep_name.clone(), filtered.clone());
                    candidates = filtered;
                }
                if candidates.is_empty() {
                    anyhow::bail!("no AUR package found for {dep_string} (required by {parent})");
                }
                candidates.sort_by(|a, b| {
                    b.popularity
                        .partial_cmp(&a.popularity)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| b.num_votes.cmp(&a.num_votes))
                        .then_with(|| a.name.cmp(&b.name))
                });
                let chosen = candidates.into_iter().next().expect("non-empty candidates");
                graph.add_node(&chosen.name);
                for provide in &chosen.provides {
                    graph.add_provides(provide, &chosen.name, &chosen.version);
                }
                graph.depend_on(&parent, &chosen.name);
                upsert_aur(&mut nodes, chosen.clone(), reason);
                provider_cache.entry(dep_name).or_default().push(chosen.clone());
                if seen_for_expansion.insert(chosen.name.clone()) {
                    worklist.push_back(chosen);
                }
            }
        }
    }

    let raw_layers = graph
        .topo_layers()
        .map_err(|leftover| anyhow::anyhow!("dependency cycle: {}", leftover.join(", ")))?;
    let mut layers: Vec<BuildLayer> = Vec::new();
    for layer_names in raw_layers {
        let mut aur_infos: Vec<AurInfo> = Vec::new();
        let mut seen_bases: HashSet<String> = HashSet::new();
        let mut repo_deps: Vec<String> = Vec::new();
        for name in &layer_names {
            let Some(node) = nodes.get(name) else {
                continue;
            };
            match node.source {
                Source::Aur => {
                    if let Some(info) = &node.aur_info
                        && seen_bases.insert(info.package_base.clone())
                    {
                        aur_infos.push(info.clone());
                    }
                }
                Source::Repo => repo_deps.push(name.clone()),
            }
        }
        aur_infos.sort_by(|a, b| {
            a.package_base
                .cmp(&b.package_base)
                .then_with(|| a.name.cmp(&b.name))
        });
        repo_deps.sort();
        layers.push(BuildLayer {
            aur: aur_infos,
            repo_deps,
        });
    }

    Ok(BuildPlan {
        targets: targets.to_vec(),
        layers,
    })
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::aur::{AurClient, AurInfo};
    use crate::resolve::satisfies::{satisfies_pkg, split_dep};
    use crate::resolve::sources::{AurQuery, PackageDb};
    use crate::resolve::types::RepoPackage;
    use crate::resolve::{AlpmDb, BuildPlan};
    use std::collections::HashMap;

    #[derive(Clone)]
    struct MockPkg {
        name: String,
        version: String,
        provides: Vec<String>,
    }

    fn pkg(name: &str, version: &str) -> MockPkg {
        MockPkg { name: name.into(), version: version.into(), provides: vec![] }
    }

    fn pkg_p(name: &str, version: &str, provides: &[&str]) -> MockPkg {
        MockPkg {
            name: name.into(),
            version: version.into(),
            provides: provides.iter().map(|&s| s.into()).collect(),
        }
    }

    struct MockDb {
        local: Vec<MockPkg>,
        repo: Vec<MockPkg>,
    }

    impl MockDb {
        fn empty() -> Self {
            Self { local: vec![], repo: vec![] }
        }
        fn local(local: Vec<MockPkg>) -> Self {
            Self { local, repo: vec![] }
        }
        fn repo(repo: Vec<MockPkg>) -> Self {
            Self { local: vec![], repo }
        }
    }

    impl PackageDb for MockDb {
        fn local_satisfier_exists(&self, dep: &str) -> bool {
            self.local.iter().any(|p| satisfies_pkg(&p.name, &p.version, &p.provides, dep))
        }

        fn sync_satisfier(&self, dep: &str) -> Option<RepoPackage> {
            self.repo
                .iter()
                .find(|p| satisfies_pkg(&p.name, &p.version, &p.provides, dep))
                .map(|p| RepoPackage {
                    name: p.name.clone(),
                    version: p.version.clone(),
                })
        }
    }

    struct MockAur(HashMap<String, AurInfo>);

    fn mock_aur(pkgs: Vec<AurInfo>) -> MockAur {
        MockAur(pkgs.into_iter().map(|p| (p.name.clone(), p)).collect())
    }

    impl AurQuery for MockAur {
        fn info_many(&self, names: &[String]) -> anyhow::Result<Vec<AurInfo>> {
            Ok(names.iter().filter_map(|n| self.0.get(n).cloned()).collect())
        }

        fn search_by_provides(&self, dep: &str) -> anyhow::Result<Vec<AurInfo>> {
            let target = split_dep(dep).name;
            Ok(self
                .0
                .values()
                .filter(|p| p.name == target || p.provides.iter().any(|pr| split_dep(pr).name == target))
                .cloned()
                .collect())
        }
    }

    fn aur(name: &str, version: &str, deps: &[&str], makedeps: &[&str], checkdeps: &[&str], provides: &[&str]) -> AurInfo {
        AurInfo {
            id: 0,
            name: name.into(),
            package_base_id: 0,
            package_base: name.into(),
            version: version.into(),
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
            depends: deps.iter().map(|&s| s.into()).collect(),
            make_depends: makedeps.iter().map(|&s| s.into()).collect(),
            check_depends: checkdeps.iter().map(|&s| s.into()).collect(),
            opt_depends: vec![],
            conflicts: vec![],
            provides: provides.iter().map(|&s| s.into()).collect(),
            replaces: vec![],
            groups: vec![],
            license: vec![],
            keywords: vec![],
            co_maintainers: vec![],
        }
    }

    fn aur_names(plan: &BuildPlan) -> Vec<String> {
        let mut names: Vec<String> = plan
            .layers
            .iter()
            .flat_map(|l| l.aur.iter().map(|p| p.name.clone()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn resolve_repo_dep() {
        let db = MockDb::repo(vec![pkg("glibc", "2.0")]);
        let aur = mock_aur(vec![aur("myaur", "1.0", &["glibc"], &[], &[], &[])]);
        let plan = resolve(&db, &aur, &["myaur".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 2);
        assert_eq!(plan.layers[0].repo_deps, vec!["glibc".to_string()]);
        assert!(plan.layers[0].aur.is_empty());
        assert_eq!(plan.layers[1].aur[0].name, "myaur");
    }

    #[test]
    fn resolve_aur_chain() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![
            aur("a", "1.0", &["b"], &[], &[], &[]),
            aur("b", "1.0", &[], &[], &[], &[]),
        ]);
        let plan = resolve(&db, &aur, &["a".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 2);
        assert_eq!(plan.layers[0].aur[0].name, "b");
        assert_eq!(plan.layers[1].aur[0].name, "a");
    }

    #[test]
    fn resolve_provides_local() {
        let db = MockDb::local(vec![pkg_p("something", "1.0", &["libfoo"])]);
        let aur = mock_aur(vec![aur("t", "1.0", &["libfoo"], &[], &[], &[])]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 1);
        assert_eq!(plan.layers[0].aur.len(), 1);
    }

    #[test]
    fn resolve_provides_repo() {
        let db = MockDb::repo(vec![pkg_p("libfoo-pkg", "1.0", &["libfoo"])]);
        let aur = mock_aur(vec![aur("t", "1.0", &["libfoo"], &[], &[], &[])]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 2);
        assert_eq!(plan.layers[0].repo_deps, vec!["libfoo-pkg".to_string()]);
        assert_eq!(plan.layers[1].aur.len(), 1);
    }

    #[test]
    fn resolve_provides_aur_recursed() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![
            aur("t", "1.0", &["libfoo"], &[], &[], &[]),
            aur("libfoo-provider", "1.0", &[], &[], &[], &["libfoo"]),
        ]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 2);
        assert_eq!(plan.layers[0].aur[0].name, "libfoo-provider");
        assert_eq!(aur_names(&plan), vec!["libfoo-provider".to_string(), "t".to_string()]);
    }

    #[test]
    fn resolve_provides_cycle_terminates() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![
            aur("t", "1.0", &["x"], &[], &[], &[]),
            aur("p", "1.0", &["x"], &[], &[], &["x"]),
        ]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 2);
        assert_eq!(plan.layers[0].aur[0].name, "p");
        assert_eq!(plan.layers[1].aur[0].name, "t");
    }

    #[test]
    fn resolve_versioned_dep() {
        for (provider_version, expect_ok) in [("2.5", true), ("1.5", false)] {
            let db = MockDb::empty();
            let aur = mock_aur(vec![
                aur("t", "1.0", &["lib>=2.0"], &[], &[], &[]),
                aur("lib", provider_version, &[], &[], &[], &[]),
            ]);
            let res = resolve(&db, &aur, &["t".to_string()], false);
            assert_eq!(res.is_ok(), expect_ok);
        }

        let db = MockDb::empty();
        let aur = mock_aur(vec![
            aur("t", "1.0", &["lib>=2.0"], &[], &[], &[]),
            aur("lib", "2.5", &[], &[], &[], &[]),
        ]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(plan.layers[0].aur[0].version, "2.5");
    }

    #[test]
    fn resolve_versioned_on_resolved_node() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![
            aur("t", "1.0", &["N", "N>=2.0"], &[], &[], &[]),
            aur("N", "1.0", &[], &[], &[], &[]),
            aur("p", "1.0", &[], &[], &[], &["N=2.5"]),
        ]);
        let plan = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        let names = aur_names(&plan);
        assert!(names.contains(&"N".to_string()));
        assert!(names.contains(&"p".to_string()));
    }

    #[test]
    fn resolve_no_check() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![aur("t", "1.0", &[], &[], &["checkpkg"], &[])]);
        let plan = resolve(&db, &aur, &["t".to_string()], true).unwrap();
        assert_eq!(plan.layers.len(), 1);
        assert_eq!(plan.layers[0].aur.len(), 1);
    }

    #[test]
    fn resolve_missing_dep() {
        let db = MockDb::empty();
        let aur = mock_aur(vec![aur("t", "1.0", &["ghost"], &[], &[], &[])]);
        let res = resolve(&db, &aur, &["t".to_string()], false);
        assert!(res.is_err());
    }

    #[test]
    fn resolve_split_package_dedup() {
        let db = MockDb::empty();
        let mut a = aur("a", "1.0", &[], &[], &[], &[]);
        a.package_base = "baseX".into();
        let mut b = aur("b", "1.0", &[], &[], &[], &[]);
        b.package_base = "baseX".into();
        let aur = mock_aur(vec![a, b]);
        let plan = resolve(&db, &aur, &["a".to_string(), "b".to_string()], false).unwrap();
        assert_eq!(plan.layers.len(), 1);
        assert_eq!(plan.layers[0].aur.len(), 1);
    }

    #[test]
    fn resolve_determinism() {
        let db = MockDb::empty();
        let mut lib_a = aur("lib-a", "1.0", &[], &[], &[], &["lib"]);
        lib_a.popularity = 5.0;
        let mut lib_b = aur("lib-b", "1.0", &[], &[], &[], &["lib"]);
        lib_b.popularity = 10.0;
        let aur = mock_aur(vec![
            aur("t", "1.0", &["lib"], &[], &[], &[]),
            lib_a,
            lib_b,
        ]);
        let plan1 = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        let plan2 = resolve(&db, &aur, &["t".to_string()], false).unwrap();
        assert_eq!(format!("{plan1:?}"), format!("{plan2:?}"));
        assert_eq!(plan1.layers[0].aur[0].name, "lib-b");
    }

    #[test]
    #[ignore]
    fn live_google_chrome() {
        let config = pacmanconf::Config::new().unwrap();
        let handle = crate::pacman::init_alpm(&config).unwrap();
        let plan = resolve(
            &AlpmDb(&handle),
            &AurClient::new(),
            &["google-chrome".to_string()],
            false,
        )
        .unwrap();
        assert!(!plan.layers.is_empty());
    }
}
