use crate::question::source::AnswerSource;
use crate::tx::driver::{RunKind, RunOutcome, RunSpec};
use std::fs;

pub struct OfflinePkg {
    pub name: &'static str,
    pub depends: &'static [&'static str],
    pub provides: &'static [&'static str],
    pub conflicts: &'static [&'static str],
    pub groups: &'static [&'static str],
}

pub fn offline_pkg(name: &'static str) -> OfflinePkg {
    OfflinePkg {
        name,
        depends: &[],
        provides: &[],
        conflicts: &[],
        groups: &[],
    }
}

pub fn offline_root(packages: &[OfflinePkg]) -> (tempfile::TempDir, alpm::Alpm) {
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
    let file = fs::File::create(db.join("sync").join("core.db")).unwrap();
    let mut builder = tar::Builder::new(file);
    for package in packages {
        let content = offline_desc(package);
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{}-1.0-1/desc", package.name),
                content.as_slice(),
            )
            .unwrap();
    }
    builder.into_inner().unwrap();
    for package in packages {
        crate::tx::targets::write_cachedir_stub(
            &cache,
            package.name,
            "1.0-1",
            &crate::tx::targets::StubLists {
                depends: package.depends,
                provides: package.provides,
                conflicts: package.conflicts,
                groups: package.groups,
                ..Default::default()
            },
        );
    }
    handle
        .register_syncdb_mut("core", alpm::SigLevel::NONE)
        .unwrap()
        .add_server("file:///pakajo-offline-stub")
        .unwrap();
    handle
        .add_cachedir(cache.to_string_lossy().as_ref())
        .unwrap();
    (dir, handle)
}

fn offline_desc(package: &OfflinePkg) -> Vec<u8> {
    let mut out = format!(
        "%NAME%\n{}\n\n%VERSION%\n1.0-1\n\n%FILENAME%\n{}\n\n",
        package.name,
        crate::tx::targets::filename(package.name, "1.0-1"),
    );
    for (tag, entries) in [
        ("%DEPENDS%\n", package.depends),
        ("%CONFLICTS%\n", package.conflicts),
        ("%PROVIDES%\n", package.provides),
        ("%GROUPS%\n", package.groups),
    ] {
        if entries.is_empty() {
            continue;
        }
        out.push_str(tag);
        for entry in entries {
            out.push_str(entry);
            out.push('\n');
        }
        out.push('\n');
    }
    out.into_bytes()
}

struct Discard;

impl crate::events::InstallSink for Discard {
    fn event(&mut self, _event: crate::events::InstallEvent) {}
}

pub fn drive_sync(
    handle: &mut alpm::Alpm,
    targets: &[&str],
    source: Box<dyn AnswerSource>,
) -> anyhow::Result<RunOutcome> {
    let spec = RunSpec {
        kind: RunKind::Sync,
        targets: targets.iter().map(|target| target.to_string()).collect(),
        stub_targets: Vec::new(),
        explore: false,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    };
    crate::tx::driver::run(handle, &spec, source, Box::new(Discard))
}
