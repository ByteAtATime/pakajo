use alpm::Alpm;

pub struct ResolvedTargets<'a> {
    pub packages: Vec<&'a alpm::Package>,
    pub skipped: Vec<String>,
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
            anyhow::bail!("database not found: {repo}");
        };
        let saved = db.usage()?;
        db.set_usage(saved | alpm::Usage::INSTALL)?;
        let mut dbs = alpm::AlpmListMut::<&alpm::Db>::new();
        dbs.push(db);
        let found = dbs.list().find_satisfier(name);
        db.set_usage(saved)?;
        if found.is_none() && !is_ignored(handle) {
            anyhow::bail!("target not found: {name}");
        }
        return Ok(found);
    }
    if let Some(pkg) = handle.syncdbs().find_satisfier(target) {
        return Ok(Some(pkg));
    }
    if is_ignored(handle) {
        return Ok(None);
    }
    anyhow::bail!("target not found: {target}");
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
    if repo.is_empty() { None } else { Some((repo, name)) }
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

    fn err(handle: &Alpm, targets: &[String]) -> String {
        match resolve_targets(handle, targets) {
            Ok(_) => panic!("expected resolution to fail"),
            Err(error) => format!("{error:#}"),
        }
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
        let resolved = resolve_targets(&handle, &["repob/dup".to_string()]).unwrap();
        assert_eq!(resolved.packages[0].version(), "2.0-1");
        for query in ["dup>=2", "repob/dup>=2"] {
            let resolved = resolve_targets(&handle, &[query.to_string()]).unwrap();
            assert_eq!(resolved.packages[0].version(), "2.0-1");
        }
        assert_eq!(
            err(&handle, &["repob/dup>=3".to_string()]),
            "target not found: dup>=3"
        );
        assert_eq!(err(&handle, &["ghost".to_string()]), "target not found: ghost");
        assert_eq!(
            err(&handle, &["nope/dup".to_string()]),
            "database not found: nope"
        );
        assert_eq!(
            err(&handle, &["repoa/ghost".to_string()]),
            "target not found: ghost"
        );
        assert_eq!(
            err(&handle, &["repoa/onlyb".to_string()]),
            "target not found: onlyb"
        );
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
        assert_eq!(err(&handle, &["virt".to_string()]), "target not found: virt");
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
        assert_eq!(
            err(&handle, &["onlyb".to_string()]),
            "target not found: onlyb"
        );
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
