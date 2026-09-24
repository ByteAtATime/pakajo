use crate::events::InstallSink;
use crate::question::model::{Answer, Question};
use crate::question::source::{AnswerSource, FailClosed, SourceDecision};
use crate::tx::driver::{RemoveSpec, RunKind, RunOutcome, RunSpec, run};
use crate::tx::targets::{StubLists, filename, write_cachedir_stub};
use std::fs::{self, File};
use std::path::Path;

#[derive(Clone)]
pub struct Pkg {
    pub name: &'static str,
    pub version: &'static str,
    pub depends: Vec<&'static str>,
    pub provides: Vec<&'static str>,
    pub conflicts: Vec<&'static str>,
    pub replaces: Vec<&'static str>,
    pub groups: Vec<&'static str>,
}

impl Pkg {
    pub fn plain(name: &'static str) -> Pkg {
        Pkg {
            name,
            version: "1.0-1",
            depends: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            groups: Vec::new(),
        }
    }

    pub fn make(
        name: &'static str,
        version: &'static str,
        depends: &[&'static str],
        provides: &[&'static str],
        groups: &[&'static str],
    ) -> Pkg {
        Pkg {
            name,
            version,
            depends: depends.to_vec(),
            provides: provides.to_vec(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            groups: groups.to_vec(),
        }
    }
}

fn push_tag_list(out: &mut String, tag: &str, entries: &[&'static str]) {
    if entries.is_empty() {
        return;
    }
    out.push_str(tag);
    out.push('\n');
    for entry in entries {
        out.push_str(entry);
        out.push('\n');
    }
    out.push('\n');
}

fn desc(package: &Pkg) -> Vec<u8> {
    let mut out = format!(
        "%NAME%\n{}\n\n%VERSION%\n{}\n\n%FILENAME%\n{}\n\n",
        package.name,
        package.version,
        filename(package.name, package.version),
    );
    push_tag_list(&mut out, "%DEPENDS%", &package.depends);
    push_tag_list(&mut out, "%CONFLICTS%", &package.conflicts);
    push_tag_list(&mut out, "%REPLACES%", &package.replaces);
    push_tag_list(&mut out, "%PROVIDES%", &package.provides);
    push_tag_list(&mut out, "%GROUPS%", &package.groups);
    out.into_bytes()
}

fn append_desc(builder: &mut tar::Builder<File>, entry: &str, content: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, entry, content).unwrap();
}

fn write_syncdb(dbpath: &Path, repo: &str, packages: &[Pkg]) {
    let file = File::create(dbpath.join("sync").join(format!("{repo}.db"))).unwrap();
    let mut builder = tar::Builder::new(file);
    for package in packages {
        let content = desc(package);
        let entry = format!("{}-{}/desc", package.name, package.version);
        append_desc(&mut builder, &entry, &content);
    }
    builder.into_inner().unwrap();
}

pub fn fixture_full(sync: &[(&str, Vec<Pkg>)], local: &[Pkg]) -> (tempfile::TempDir, alpm::Alpm) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let db = dir.path().join("db");
    let cache = dir.path().join("cache");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(db.join("local")).unwrap();
    fs::create_dir_all(db.join("sync")).unwrap();
    fs::create_dir_all(&cache).unwrap();
    let mut handle = alpm::Alpm::new(
        root.to_string_lossy().as_ref(),
        db.to_string_lossy().as_ref(),
    )
    .unwrap();
    for (repo, packages) in sync {
        write_syncdb(&db, repo, packages);
        for package in packages {
            write_cachedir_stub(
                &cache,
                package.name,
                package.version,
                &StubLists {
                    depends: &package.depends,
                    provides: &package.provides,
                    conflicts: &package.conflicts,
                    replaces: &package.replaces,
                    groups: &package.groups,
                },
            );
        }
    }
    for package in local {
        let dir_path = db
            .join("local")
            .join(format!("{}-{}", package.name, package.version));
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(dir_path.join("desc"), desc(package)).unwrap();
        fs::write(dir_path.join("files"), b"%FILES%\n").unwrap();
    }
    for (repo, _) in sync {
        handle
            .register_syncdb_mut(*repo, alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
    }
    handle
        .add_cachedir(cache.to_string_lossy().as_ref())
        .unwrap();
    (dir, handle)
}

pub fn fixture(packages: &[Pkg]) -> (tempfile::TempDir, alpm::Alpm) {
    fixture_full(&[("core", packages.to_vec())], &[])
}

fn base_spec(kind: RunKind, targets: &[&str], explore: bool) -> RunSpec {
    RunSpec {
        kind,
        targets: targets.iter().map(|t| t.to_string()).collect(),
        stub_targets: Vec::new(),
        explore,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    }
}

pub fn sync_spec(targets: &[&str], explore: bool) -> RunSpec {
    base_spec(RunKind::Sync, targets, explore)
}

pub fn upgrade_spec(targets: &[&str], explore: bool) -> RunSpec {
    base_spec(RunKind::Upgrade, targets, explore)
}

pub fn remove_spec(flags: alpm::TransFlag, explore: bool, targets: &[&str]) -> RunSpec {
    base_spec(
        RunKind::Remove(RemoveSpec {
            flags,
            holds: Vec::new(),
        }),
        targets,
        explore,
    )
}

pub fn script(respond: impl Fn(&Question) -> SourceDecision + 'static) -> Box<dyn AnswerSource> {
    struct Script {
        respond: Box<dyn Fn(&Question) -> SourceDecision>,
    }

    impl AnswerSource for Script {
        fn answer(&self, question: &Question) -> SourceDecision {
            (self.respond)(question)
        }
    }

    Box::new(Script {
        respond: Box::new(respond),
    })
}

pub fn deny() -> Box<dyn AnswerSource> {
    script(|question| {
        SourceDecision::Abort(FailClosed {
            key: question.key(),
            reason: "denied in test".to_string(),
        })
    })
}

pub fn stop() -> Box<dyn AnswerSource> {
    script(|_| SourceDecision::Answer(Answer::Stop))
}

pub fn proceed() -> Box<dyn AnswerSource> {
    script(|question| match question {
        Question::Proceed { .. } => SourceDecision::Answer(Answer::Proceed),
        _ => SourceDecision::Answer(Answer::Stop),
    })
}

pub fn discard() -> Box<dyn InstallSink> {
    Box::new(crate::events::DiscardSink)
}

pub fn drive_sync(
    handle: &mut alpm::Alpm,
    targets: &[&str],
    source: Box<dyn AnswerSource>,
) -> anyhow::Result<RunOutcome> {
    run(handle, &sync_spec(targets, false), source, discard())
}

pub fn summary_names(outcome: &RunOutcome) -> Vec<String> {
    let mut names: Vec<String> = outcome
        .summary
        .packages
        .iter()
        .map(|package| package.name.clone())
        .collect();
    names.sort();
    names
}
