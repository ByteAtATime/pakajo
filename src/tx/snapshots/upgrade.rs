use super::{approved, installed_names, snapshot_settings, summary_package_lines};
use crate::dispatch::sysupgrade::upgrade_review;
use crate::question::model::{Answer, Question};
use crate::question::source::{AnswerSource, ExploreDefaults, SourceDecision};

struct SyncPkg {
    name: &'static str,
    version: &'static str,
    extra: &'static str,
}

struct LocalPkg {
    name: &'static str,
    version: &'static str,
}

fn sync_pkg(name: &'static str, version: &'static str) -> SyncPkg {
    SyncPkg {
        name,
        version,
        extra: "",
    }
}

fn local_pkg(name: &'static str, version: &'static str) -> LocalPkg {
    LocalPkg { name, version }
}

fn upgrade_fixture(local: &[LocalPkg], sync: &[SyncPkg]) -> (tempfile::TempDir, alpm::Alpm) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let db = dir.path().join("db");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(db.join("local")).unwrap();
    std::fs::create_dir_all(db.join("sync")).unwrap();
    for entry in local {
        let entry_dir = db
            .join("local")
            .join(format!("{}-{}", entry.name, entry.version));
        std::fs::create_dir_all(&entry_dir).unwrap();
        let desc = format!("%NAME%\n{}\n\n%VERSION%\n{}\n\n", entry.name, entry.version);
        std::fs::write(entry_dir.join("desc"), desc).unwrap();
        std::fs::write(entry_dir.join("files"), "%FILES%\n").unwrap();
    }
    std::fs::write(db.join("local").join("ALPM_DB_VERSION"), "9").unwrap();
    let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
    let mut builder = tar::Builder::new(file);
    for entry in sync {
        let desc = format!(
            "%NAME%\n{}\n\n%VERSION%\n{}\n\n%FILENAME%\n{}-{}-x86_64.pkg.tar.zst\n\n{}",
            entry.name, entry.version, entry.name, entry.version, entry.extra
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(desc.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{}-{}/desc", entry.name, entry.version),
                desc.as_bytes(),
            )
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

fn question_kind(question: &Question) -> &'static str {
    match question {
        Question::Conflict { .. } => "Conflict",
        Question::SelectProvider { .. } => "SelectProvider",
        Question::Replace { .. } => "Replace",
        Question::InstallIgnorepkg { .. } => "InstallIgnorepkg",
        Question::RemovePkgs { .. } => "RemovePkgs",
        Question::HoldPkgs { .. } => "HoldPkgs",
        Question::Corrupted { .. } => "Corrupted",
        Question::ImportKey { .. } => "ImportKey",
        Question::Proceed { .. } => "Proceed",
        Question::GroupMembers { .. } => "GroupMembers",
    }
}

fn explore_default(question: &Question) -> String {
    match ExploreDefaults.answer(question) {
        SourceDecision::Answer(answer) => match answer {
            Answer::Conflict { remove, .. } => format!("remove={remove}"),
            Answer::SelectProvider { name, repo } => {
                format!("provider={} repo={}", name, repo.as_deref().unwrap_or("-"))
            }
            Answer::Replace { replace, .. } => format!("replace={replace}"),
            Answer::InstallIgnorepkg { install, .. } => format!("install={install}"),
            Answer::RemovePkgs { skip, .. } => format!("skip={skip}"),
            Answer::HoldPkgs { proceed, .. } => format!("proceed={proceed}"),
            Answer::Corrupted { remove, .. } => format!("remove={remove}"),
            Answer::ImportKey { import, .. } => format!("import={import}"),
            Answer::Proceed => "proceed".to_string(),
            Answer::Stop => "stop".to_string(),
            Answer::GroupMembers { selected } => format!("members={}", selected.join(",")),
        },
        SourceDecision::Abort(denied) => format!("abort:{}", denied.reason),
    }
}

fn render_question(question: &Question) -> String {
    format!(
        "question {} {:?} default={}",
        question_kind(question),
        question.key(),
        explore_default(question)
    )
}

fn run_upgrade_case(local: &[LocalPkg], sync: &[SyncPkg]) -> String {
    let (_dir, mut handle) = upgrade_fixture(local, sync);
    let (assessed, finish) =
        upgrade_review(&mut handle, approved()).expect("upgrade explore review succeeds");
    let mut rendered = String::new();
    for question in &assessed.review.part1 {
        rendered.push_str(&render_question(question));
        rendered.push('\n');
    }
    rendered.push_str("summary:\n");
    for line in summary_package_lines(&assessed.review.part2) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered.push_str(&format!("finish: {finish:?}\n"));
    rendered.push_str("installed:\n");
    for line in installed_names(&handle) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered
}

#[test]
fn upgrade_plain_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_plain_snapshot",
            run_upgrade_case(
                &[local_pkg("upgun", "1.0-1")],
                &[sync_pkg("upgun", "2.0-1")],
            )
        );
    });
}

#[test]
fn upgrade_replace_conflict_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_replace_conflict_snapshot",
            run_upgrade_case(
                &[
                    local_pkg("dummy-old", "1.0-1"),
                    local_pkg("clash-a", "1.0-1"),
                    local_pkg("clash-b", "0.5-1"),
                ],
                &[
                    SyncPkg {
                        name: "dummy-new",
                        version: "2.0-1",
                        extra: "%REPLACES%\ndummy-old\n\n",
                    },
                    SyncPkg {
                        name: "clash-b",
                        version: "1.0-1",
                        extra: "%CONFLICTS%\nclash-a\n\n",
                    },
                ],
            )
        );
    });
}

#[test]
fn upgrade_provider_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_provider_snapshot",
            run_upgrade_case(
                &[local_pkg("needsvirt", "1.0-1")],
                &[
                    SyncPkg {
                        name: "needsvirt",
                        version: "2.0-1",
                        extra: "%DEPENDS%\nvirt\n\n",
                    },
                    SyncPkg {
                        name: "provider-one",
                        version: "1.0-1",
                        extra: "%PROVIDES%\nvirt\n\n",
                    },
                    SyncPkg {
                        name: "provider-two",
                        version: "1.0-1",
                        extra: "%PROVIDES%\nvirt\n\n",
                    },
                ],
            )
        );
    });
}

#[test]
fn upgrade_nothing_to_do_snapshot() {
    snapshot_settings().bind(|| {
        insta::assert_snapshot!(
            "upgrade_nothing_to_do_snapshot",
            run_upgrade_case(
                &[local_pkg("upgun", "1.0-1")],
                &[sync_pkg("upgun", "1.0-1")],
            )
        );
    });
}
