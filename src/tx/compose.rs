use crate::question::source::ExploreDefaults;
use crate::tx::driver::{RunOutcome, RunSpec, run};

struct DiscardSink;

impl crate::events::InstallSink for DiscardSink {
    fn event(&mut self, _event: crate::events::InstallEvent) {}
}

pub fn preview(handle: &mut alpm::Alpm, mut spec: RunSpec) -> anyhow::Result<RunOutcome> {
    spec.explore = true;
    run(
        handle,
        &spec,
        Box::new(ExploreDefaults),
        Box::new(DiscardSink),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::driver::{Finish, RunKind};

    fn spec(targets: &[&str]) -> RunSpec {
        RunSpec {
            kind: RunKind::Sync,
            targets: targets.iter().map(|t| t.to_string()).collect(),
            stub_targets: Vec::new(),
            explore: true,
            as_deps: false,
            reinstall: false,
        }
    }

    fn fixture(packages: &[&'static str]) -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        for name in packages {
            let desc = format!(
                "%NAME%\n{name}\n\n%VERSION%\n1.0-1\n\n%FILENAME%\n{name}-1.0-1-x86_64.pkg.tar.zst\n\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(desc.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{name}-1.0-1/desc"), desc.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        handle
            .register_syncdb_mut("core", alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
        (dir, handle)
    }

    #[test]
    fn preview_explores_with_explore_defaults_and_returns_review() {
        let (_dir, mut handle) = fixture(&["solo"]);
        let outcome = preview(&mut handle, spec(&["solo"])).unwrap();
        assert!(matches!(outcome.finish, Finish::Stopped));
        let review = outcome.review.expect("explore run carries a review");
        assert_eq!(
            review
                .part2
                .packages
                .iter()
                .map(|pkg| pkg.name.clone())
                .collect::<Vec<_>>(),
            vec!["solo".to_string()]
        );
        assert!(review.part1.is_empty());
        assert_eq!(review.generated_by.dbs.get("core").unwrap().packages, 1);
        assert!(handle.localdb().pkg("solo").is_err());
        handle.trans_init(alpm::TransFlag::NONE).unwrap();
        handle.trans_release().unwrap();
    }
}
