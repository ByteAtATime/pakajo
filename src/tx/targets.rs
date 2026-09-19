use alpm::Alpm;

#[derive(Debug)]
pub enum ResolveError {
    TargetNotFound(String),
    DatabaseNotFound(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::TargetNotFound(name) => write!(formatter, "target not found: {name}"),
            ResolveError::DatabaseNotFound(repo) => write!(formatter, "database not found: {repo}"),
        }
    }
}

impl std::error::Error for ResolveError {}

pub struct ResolvedTargets<'a> {
    pub packages: Vec<&'a alpm::Package>,
    pub skipped: Vec<String>,
}

pub fn unresolvable_target(handle: &Alpm, targets: &[String]) -> Option<String> {
    for target in targets {
        let Err(error) = resolve_targets(handle, std::slice::from_ref(target)) else {
            continue;
        };
        if error.downcast_ref::<ResolveError>().is_some() {
            return Some(target.clone());
        }
    }
    None
}

pub fn resolve_targets<'a>(
    handle: &'a Alpm,
    targets: &[String],
) -> anyhow::Result<ResolvedTargets<'a>> {
    let mut packages = Vec::with_capacity(targets.len());
    let mut skipped = Vec::new();
    for target in targets {
        match resolve_one(handle, target)? {
            Some(pkg) => packages.push(pkg),
            None => skipped.push(bare_name(target).to_string()),
        }
    }
    Ok(ResolvedTargets { packages, skipped })
}

fn resolve_one<'a>(handle: &'a Alpm, target: &str) -> anyhow::Result<Option<&'a alpm::Package>> {
    if let Some((repo, name)) = split_pin(target) {
        let Some(db) = handle.syncdbs().iter().find(|db| db.name() == repo) else {
            return Err(ResolveError::DatabaseNotFound(repo.to_string()).into());
        };
        let saved = db.usage()?;
        db.set_usage(saved | alpm::Usage::INSTALL)?;
        let mut dbs = alpm::AlpmListMut::<&alpm::Db>::new();
        dbs.push(db);
        let found = dbs.list().find_satisfier(name);
        db.set_usage(saved)?;
        if found.is_none() && !is_ignored(handle) {
            return Err(ResolveError::TargetNotFound(name.to_string()).into());
        }
        return Ok(found);
    }
    if let Some(pkg) = handle.syncdbs().find_satisfier(target) {
        return Ok(Some(pkg));
    }
    if is_ignored(handle) {
        return Ok(None);
    }
    Err(ResolveError::TargetNotFound(target.to_string()).into())
}

fn is_ignored(handle: &Alpm) -> bool {
    handle.last_error() == alpm::Error::PkgIgnored
}

fn bare_name(target: &str) -> &str {
    match split_pin(target) {
        Some((_, name)) => name,
        None => target,
    }
}

fn split_pin(target: &str) -> Option<(&str, &str)> {
    let (repo, name) = target.split_once('/')?;
    if repo.is_empty() {
        None
    } else {
        Some((repo, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs::File;
    use std::path::Path;
    use std::rc::Rc;

    struct FixturePackage {
        name: &'static str,
        version: &'static str,
        provides: &'static [&'static str],
    }

    fn pkg(name: &'static str, version: &'static str) -> FixturePackage {
        FixturePackage {
            name,
            version,
            provides: &[],
        }
    }

    fn desc(name: &str, version: &str, provides: &[&str]) -> Vec<u8> {
        let mut out = format!(
            "%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}-{version}-x86_64.pkg.tar.zst\n\n"
        );
        if !provides.is_empty() {
            out.push_str("%PROVIDES%\n");
            for provide in provides {
                out.push_str(provide);
                out.push('\n');
            }
            out.push('\n');
        }
        out.into_bytes()
    }

    fn write_syncdb(dbpath: &Path, repo: &str, packages: &[FixturePackage]) {
        let file = File::create(dbpath.join("sync").join(format!("{repo}.db"))).unwrap();
        let mut builder = tar::Builder::new(file);
        for package in packages {
            let content = desc(package.name, package.version, package.provides);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{}-{}/desc", package.name, package.version),
                    content.as_slice(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
    }

    fn fixture(
        repos: &[(&str, Vec<FixturePackage>)],
        local: &[FixturePackage],
    ) -> (tempfile::TempDir, Alpm) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("local")).unwrap();
        std::fs::create_dir_all(dir.path().join("sync")).unwrap();
        let handle = Alpm::new("/", dir.path().to_string_lossy().as_ref()).unwrap();
        for (repo, packages) in repos {
            write_syncdb(dir.path(), repo, packages);
        }
        for package in local {
            let dir_path = dir
                .path()
                .join("local")
                .join(format!("{}-{}", package.name, package.version));
            std::fs::create_dir_all(&dir_path).unwrap();
            std::fs::write(
                dir_path.join("desc"),
                desc(package.name, package.version, package.provides),
            )
            .unwrap();
        }
        for (repo, _) in repos {
            handle.register_syncdb(*repo, alpm::SigLevel::NONE).unwrap();
        }
        (dir, handle)
    }

    fn err(handle: &Alpm, target: &str) -> String {
        match resolve_targets(handle, &[target.to_string()]) {
            Ok(_) => panic!("expected resolution to fail"),
            Err(error) => format!("{error:#}"),
        }
    }

    fn lookup(handle: &Alpm, target: &str) -> (String, String) {
        let resolved = resolve_targets(handle, &[target.to_string()]).unwrap();
        let pkg = resolved.packages[0];
        (
            pkg.version().to_string(),
            pkg.db().unwrap().name().to_string(),
        )
    }

    fn providers() -> Vec<FixturePackage> {
        vec![
            FixturePackage {
                name: "provider-one",
                version: "1.0-1",
                provides: &["virt"],
            },
            FixturePackage {
                name: "provider-two",
                version: "1.0-1",
                provides: &["virt"],
            },
        ]
    }

    fn unresolvable(handle: &Alpm, targets: &[&str]) -> Option<String> {
        let owned: Vec<String> = targets.iter().map(|t| t.to_string()).collect();
        unresolvable_target(handle, &owned)
    }

    #[test]
    fn unresolvable_target_probes_pins_skips_and_first_failure() {
        let (_dir, handle) = fixture(
            &[
                ("core", vec![pkg("foo", "1.0-1")]),
                ("extra", vec![pkg("foo", "2.0-1")]),
            ],
            &[],
        );
        assert_eq!(unresolvable(&handle, &["core/foo"]), None);
        assert_eq!(unresolvable(&handle, &["ghost"]), Some("ghost".to_string()));
        assert_eq!(
            unresolvable(&handle, &["nope/foo"]),
            Some("nope/foo".to_string())
        );
        assert_eq!(
            unresolvable(&handle, &["core/ghost"]),
            Some("core/ghost".to_string())
        );
        assert_eq!(
            unresolvable(&handle, &["foo", "ghost", "nope/foo"]),
            Some("ghost".to_string())
        );
        let (_dir, handle) = fixture(&[("repoa", providers())], &[]);
        assert_eq!(unresolvable(&handle, &["virt"]), None);
        let (_dir, mut handle) = fixture(&[("repoa", vec![pkg("skipme", "1.0-1")])], &[]);
        handle.add_ignorepkg("skipme").unwrap();
        handle.set_question_cb((), |question, _| {
            if let alpm::Question::InstallIgnorepkg(mut iq) = question.question() {
                iq.set_install(false);
            }
        });
        assert_eq!(unresolvable(&handle, &["skipme"]), None);
    }

    #[test]
    fn literals_pins_and_versioned_queries() {
        let (_dir, handle) = fixture(
            &[
                ("repoa", vec![pkg("dup", "1.0-1"), pkg("common", "1.0-1")]),
                ("repob", vec![pkg("dup", "2.0-1"), pkg("onlyb", "1.0-1")]),
            ],
            &[],
        );
        let resolved = resolve_targets(&handle, &["dup".to_string()]).unwrap();
        assert_eq!(resolved.packages[0].version(), "1.0-1");
        assert_eq!(resolved.packages[0].db().unwrap().name(), "repoa");
        assert_eq!(
            lookup(&handle, "repob/dup"),
            ("2.0-1".to_string(), "repob".to_string())
        );
        for query in ["dup>=2", "repob/dup>=2"] {
            assert_eq!(lookup(&handle, query).0, "2.0-1");
        }
        assert_eq!(err(&handle, "repob/dup>=3"), "target not found: dup>=3");
        assert_eq!(err(&handle, "ghost"), "target not found: ghost");
        assert_eq!(err(&handle, "nope/dup"), "database not found: nope");
        assert_eq!(err(&handle, "repoa/ghost"), "target not found: ghost");
        assert_eq!(err(&handle, "repoa/onlyb"), "target not found: onlyb");
    }

    #[test]
    fn provider_selection() {
        let (_dir, handle) = fixture(&[("repoa", providers())], &[pkg("provider-one", "1.0-1")]);
        let asked: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        handle.set_question_cb(asked.clone(), |question, data| {
            if matches!(question.question(), alpm::Question::SelectProvider(_)) {
                *data.borrow_mut() = true;
            }
        });
        let resolved = resolve_targets(&handle, &["virt".to_string()]).unwrap();
        assert_eq!(resolved.packages[0].name(), "provider-one");
        assert!(!*asked.borrow());

        let (_dir, handle) = fixture(&[("repoa", providers())], &[]);
        handle.set_question_cb((), |question, _| {
            if let alpm::Question::SelectProvider(mut spq) = question.question() {
                spq.set_index(1);
            }
        });
        let resolved = resolve_targets(&handle, &["virt".to_string()]).unwrap();
        assert_eq!(resolved.packages[0].name(), "provider-two");

        let (_dir, handle) = fixture(&[("repoa", providers())], &[]);
        handle.set_question_cb((), |question, _| {
            if let alpm::Question::SelectProvider(mut spq) = question.question() {
                spq.set_index(-1);
            }
        });
        assert_eq!(err(&handle, "virt"), "target not found: virt");
    }

    #[test]
    fn search_gated_db_needs_a_pin_and_restores_usage() {
        let (_dir, handle) = fixture(
            &[
                ("repoa", vec![pkg("common", "1.0-1")]),
                ("repob", vec![pkg("onlyb", "1.0-1")]),
            ],
            &[],
        );
        let gated = handle
            .syncdbs()
            .iter()
            .find(|db| db.name() == "repob")
            .unwrap();
        gated.set_usage(alpm::Usage::SEARCH).unwrap();
        assert_eq!(err(&handle, "onlyb"), "target not found: onlyb");
        let resolved = resolve_targets(&handle, &["repob/onlyb".to_string()]).unwrap();
        assert_eq!(resolved.packages[0].name(), "onlyb");
        assert_eq!(gated.usage().unwrap(), alpm::Usage::SEARCH);
    }

    #[test]
    fn ignored_targets_are_skipped_under_bare_name() {
        let (_dir, mut handle) = fixture(&[("repoa", vec![pkg("skipme", "1.0-1")])], &[]);
        handle.add_ignorepkg("skipme").unwrap();
        let asked: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        handle.set_question_cb(asked.clone(), |question, data| {
            if matches!(question.question(), alpm::Question::InstallIgnorepkg(_)) {
                *data.borrow_mut() = true;
            }
        });
        for query in ["skipme", "repoa/skipme"] {
            let resolved = resolve_targets(&handle, &[query.to_string()]).unwrap();
            assert!(resolved.packages.is_empty());
            assert_eq!(resolved.skipped, vec!["skipme".to_string()]);
        }
        assert!(*asked.borrow());
    }
}
