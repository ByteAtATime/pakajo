use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::Stdio;
use std::rc::Rc;

use anyhow::Context as _;
use futures::channel::mpsc;

use crate::aur::AurInfo;
use crate::cli::escalation_command;
use crate::events::{InstallEvent, InstallSink, LogLevel, TransactionSummary, summaries_match};
use crate::install::{
    ChildOutcome, QuestionState, StreamItem, build_summary, map_outcome, register_callbacks,
};
use crate::resolve::AurQuery;

pub(crate) fn write_fingerprint_file(summary_bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "pakajo-sysupgrade-fingerprint-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, summary_bytes)?;
    Ok(path)
}

fn read_fingerprint_file(path: &str) -> anyhow::Result<TransactionSummary> {
    let bytes = std::fs::read(path).context("failed to read fingerprint file")?;
    let _ = std::fs::remove_file(path);
    serde_json::from_slice(&bytes).context("fingerprint file is not valid json")
}

pub fn run_repo_sysupgrade<S: InstallSink + 'static>(
    no_refresh: bool,
    extra_ignores: &[String],
    sink: S,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
    fingerprint_file: Option<&str>,
) -> anyhow::Result<()> {
    let preview = fingerprint_file.map(read_fingerprint_file).transpose()?;
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::pacman::init_alpm(&config)?;
    apply_ignores(&mut handle, &config, extra_ignores);
    if !no_refresh {
        handle
            .syncdbs_mut()
            .update(false)
            .context("failed to refresh sync DBs")?;
    }
    repo_sysupgrade_into(&mut handle, sink, answerer, preview.as_ref())
}

pub(crate) fn run_sysupgrade_process(
    exe: PathBuf,
    fingerprint_file: String,
    mut tx: mpsc::Sender<StreamItem>,
    approvals_b64: Option<String>,
) {
    let mut send_event = |mut item: StreamItem| loop {
        match tx.try_send(item) {
            Ok(()) => return,
            Err(err) => {
                if err.is_disconnected() {
                    return;
                }
                item = err.into_inner();
                std::thread::yield_now();
            }
        }
    };

    let mut cmd = escalation_command(&exe.to_string_lossy());
    cmd.arg("upgrade")
        .arg("--json")
        .arg("--repo-only")
        .arg("--fingerprint-file")
        .arg(&fingerprint_file);
    if let Some(b64) = &approvals_b64 {
        cmd.arg("--approvals").arg(b64);
    }
    let outcome = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Err(error) if error.kind() == io::ErrorKind::NotFound => ChildOutcome::NotFound,
        Err(error) => ChildOutcome::Failed(error.to_string()),
        Ok(mut child) => {
            let stdout = child.stdout.take().expect("piped");
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if let Ok(ev) = serde_json::from_str::<InstallEvent>(&l) {
                            send_event(StreamItem::Event(ev));
                        }
                    }
                    Err(_) => break,
                }
            }
            map_outcome(child.wait())
        }
    };
    send_event(StreamItem::Done(outcome));
}

pub(crate) fn apply_ignores(
    handle: &mut alpm::Alpm,
    config: &pacmanconf::Config,
    extra: &[String],
) {
    for name in &config.ignore_pkg {
        let _ = handle.add_ignorepkg(name.as_str());
    }
    for group in &config.ignore_group {
        let _ = handle.add_ignoregroup(group.as_str());
    }
    for name in extra {
        let _ = handle.add_ignorepkg(name.as_str());
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct AurUpgradeCandidate {
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

pub(crate) fn compute_aur_upgrades(
    handle: &alpm::Alpm,
    aur: &impl AurQuery,
) -> anyhow::Result<Vec<AurUpgradeCandidate>> {
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
    let foreign_names: Vec<String> = installed
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| !sync_names.contains(name))
        .collect();
    if foreign_names.is_empty() {
        return Ok(vec![]);
    }
    let infos = aur.info_many(&foreign_names)?;
    let aur_infos: HashMap<String, AurInfo> =
        infos.into_iter().map(|i| (i.name.clone(), i)).collect();
    let mut candidates = select_upgradable_candidates(installed, &sync_names, &aur_infos);

    let devel_updates = crate::devel::possible_devel_updates();
    let devel_names: HashSet<String> = devel_updates.into_iter().collect();
    let stale_devel_names: Vec<String> = devel_names
        .iter()
        .filter(|name| !candidates.iter().any(|c| &c.name == *name))
        .cloned()
        .collect();
    for name in &stale_devel_names {
        let pkg = match handle.localdb().pkg(name.as_str()) {
            Ok(pkg) => pkg,
            Err(_) => continue,
        };
        candidates.push(AurUpgradeCandidate {
            name: name.clone(),
            local_version: pkg.version().to_string(),
            remote_version: "latest-commit".to_string(),
            package_base: pkg.base().unwrap_or(name).to_string(),
        });
    }
    candidates.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(candidates)
}

fn repo_sysupgrade_into<S: InstallSink + 'static>(
    handle: &mut alpm::Alpm,
    sink: S,
    answerer: Box<dyn crate::answerer::QuestionAnswerer>,
    preview: Option<&TransactionSummary>,
) -> anyhow::Result<()> {
    let sink = Rc::new(RefCell::new(sink));
    let qstate = Rc::new(RefCell::new(QuestionState::new(answerer)));
    register_callbacks(handle, sink.clone(), qstate.clone());
    let result = run_sysupgrade_transaction(handle, &sink, &qstate, preview);
    let _ = handle.trans_release();
    result
}

fn run_sysupgrade_transaction<S: InstallSink>(
    handle: &mut alpm::Alpm,
    sink: &Rc<RefCell<S>>,
    qstate: &Rc<RefCell<QuestionState>>,
    preview: Option<&TransactionSummary>,
) -> anyhow::Result<()> {
    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;

    handle
        .sync_sysupgrade(false)
        .context("failed to populate sysupgrade targets")?;

    handle
        .trans_prepare()
        .map_err(alpm::Error::from)
        .context("failed to prepare transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    let summary: TransactionSummary = build_summary(handle);
    if let Some(prev) = preview
        && !summaries_match(prev, &summary)
    {
        sink.borrow_mut().event(InstallEvent::Log {
            level: LogLevel::Error,
            message: "review is stale; re-review the upgrade".to_string(),
        });
        anyhow::bail!("review is stale");
    }
    sink.borrow_mut()
        .event(InstallEvent::TransactionSummary(summary));

    handle
        .trans_commit()
        .map_err(alpm::Error::from)
        .context("failed to commit transaction")?;
    if qstate.borrow().deny_flag {
        anyhow::bail!("aborted: {}", qstate.borrow().detail);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConsoleSink;
    use crate::install::setup_fake_root;
    use std::collections::HashSet;

    #[test]
    #[ignore]
    fn repo_sysupgrade_into_empty() {
        let mut handle = setup_fake_root("sysupgrade_empty");
        let result = repo_sysupgrade_into(
            &mut handle,
            ConsoleSink::new(),
            Box::new(crate::answerer::DenyAllAnswerer),
            None,
        );
        result.expect("sysupgrade with nothing to do should succeed");
    }

    fn phantom_pkg(name: &str) -> crate::events::SummaryPackage {
        crate::events::SummaryPackage {
            name: name.to_string(),
            repository: None,
            new_version: "1.0".to_string(),
            old_version: None,
            download_size: 0,
            installed_size: 0,
            old_installed_size: 0,
            is_removal: false,
        }
    }

    fn empty_summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    #[test]
    fn fingerprint_round_trips() {
        let original = TransactionSummary {
            packages: vec![phantom_pkg("ghost")],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        };
        let path =
            write_fingerprint_file(&serde_json::to_vec(&original).unwrap()).expect("write file");
        let decoded =
            read_fingerprint_file(path.to_str().expect("utf8 path")).expect("read should succeed");
        assert!(summaries_match(&original, &decoded));
    }

    #[test]
    #[ignore]
    fn stale_review_accepts_matching_summary() {
        let mut handle = setup_fake_root("stale_match");
        let preview = empty_summary();
        let res = repo_sysupgrade_into(
            &mut handle,
            ConsoleSink::new(),
            Box::new(crate::answerer::DenyAllAnswerer),
            Some(&preview),
        );
        if let Err(e) = &res {
            let rendered = format!("{e:#}");
            assert!(
                !rendered.contains("stale"),
                "matching summary must not trip the staleness guard, got: {rendered}",
            );
        }
    }

    #[test]
    #[ignore]
    fn stale_review_rejects_mismatched_summary() {
        let mut handle = setup_fake_root("stale_mismatch");
        let phantom = TransactionSummary {
            packages: vec![phantom_pkg("ghost")],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        };
        let result = repo_sysupgrade_into(
            &mut handle,
            ConsoleSink::new(),
            Box::new(crate::answerer::DenyAllAnswerer),
            Some(&phantom),
        );
        assert!(result.is_err(), "stale summary should bail before commit");
        let local_count = handle.localdb().pkgs().iter().count();
        assert_eq!(local_count, 0, "trans_commit must never have run");
    }

    #[test]
    fn aur_targets_filtered_by_ignore_list() {
        let mut targets = vec![
            AurUpgradeCandidate {
                name: "foo".to_string(),
                local_version: "1.0".to_string(),
                remote_version: "1.1".to_string(),
                package_base: "foo".to_string(),
            },
            AurUpgradeCandidate {
                name: "bar".to_string(),
                local_version: "1.0".to_string(),
                remote_version: "1.1".to_string(),
                package_base: "bar".to_string(),
            },
            AurUpgradeCandidate {
                name: "baz".to_string(),
                local_version: "1.0".to_string(),
                remote_version: "1.1".to_string(),
                package_base: "baz".to_string(),
            },
        ];
        let ignores: Vec<String> = vec!["bar".to_string()];
        targets.retain(|c| !ignores.contains(&c.name));
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().all(|c| c.name != "bar"));
    }

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

    #[test]
    #[ignore]
    fn compute_aur_upgrades_live() {
        let config = pacmanconf::Config::new().expect("pacman config");
        let handle = crate::pacman::init_alpm(&config).expect("alpm");
        let aur = crate::aur::AurClient::new();
        let candidates = compute_aur_upgrades(&handle, &aur).expect("rpc succeeds");
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
