use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::events::TransactionSummary;
use crate::question::{Conflict, ProviderCandidate, ProviderPrompt, QuestionSet};

#[derive(Default)]
pub(crate) struct RecorderState {
    conflicts: Vec<Conflict>,
    providers: Vec<ProviderPrompt>,
    had_unsupported: bool,
    unsupported_summary: String,
}

pub(crate) struct SysupgradeDryRun {
    pub summary: TransactionSummary,
    pub questions: QuestionSet,
    pub prepare_error: Option<PrepareFailure>,
}

pub(crate) fn dry_sysupgrade(handle: &mut alpm::Alpm) -> anyhow::Result<SysupgradeDryRun> {
    let state = attach_recorder(handle);
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init sysupgrade preview transaction")?;
    handle
        .sync_sysupgrade(false)
        .context("sync_sysupgrade failed to resolve upgrade targets")?;
    let prepare_error = handle.trans_prepare().err().map(extract_prepare_failure);
    let questions = snapshot(&state);
    let summary = crate::install::build_summary(handle);
    let _ = handle.trans_release();
    Ok(SysupgradeDryRun {
        summary,
        questions,
        prepare_error,
    })
}

pub fn default_repo_summary(
    handle: &mut alpm::Alpm,
) -> anyhow::Result<crate::events::TransactionSummary> {
    dry_sysupgrade(handle).map(|dry| dry.summary)
}

#[derive(Debug, Clone)]
pub enum PrepareFailure {
    Unsatisfied(Vec<UnsatisfiedDep>),
    Other(String),
}

#[derive(Debug, Clone)]
pub struct UnsatisfiedDep {
    pub depend: String,
    pub target: String,
}

pub(crate) fn extract_prepare_failure(err: alpm::PrepareError) -> PrepareFailure {
    match err.data() {
        Some(alpm::PrepareData::UnsatisfiedDeps(list)) => PrepareFailure::Unsatisfied(
            list.iter()
                .map(|d| UnsatisfiedDep {
                    depend: d.depend().name().to_string(),
                    target: d.target().to_string(),
                })
                .collect(),
        ),
        Some(other) => PrepareFailure::Other(format!("{other:?}")),
        None => PrepareFailure::Other(format!("{}", err.error())),
    }
}

pub(crate) fn attach_recorder(handle: &mut alpm::Alpm) -> Rc<RefCell<RecorderState>> {
    let state = Rc::new(RefCell::new(RecorderState::default()));
    handle.set_question_cb(
        state.clone(),
        |any_question: alpm::AnyQuestion, data: &mut Rc<RefCell<RecorderState>>| {
            let mut s = data.borrow_mut();
            match any_question.question() {
                alpm::Question::Conflict(mut cq) => {
                    let c = cq.conflict();
                    s.conflicts.push(Conflict {
                        incoming: c.package1().name().to_string(),
                        removable: c.package2().name().to_string(),
                    });
                    cq.set_remove(true);
                }
                alpm::Question::Replace(rq) => {
                    rq.set_replace(true);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("replace; ");
                }
                alpm::Question::InstallIgnorepkg(mut iq) => {
                    iq.set_install(false);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("install-ignorepkg; ");
                }
                alpm::Question::Corrupted(mut cq) => {
                    cq.set_remove(true);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("corrupted; ");
                }
                alpm::Question::RemovePkgs(mut rq) => {
                    rq.set_skip(false);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("remove-pkgs; ");
                }
                alpm::Question::SelectProvider(mut spq) => {
                    let depend = spq.depend().to_string();
                    let candidates: Vec<ProviderCandidate> = spq
                        .providers()
                        .into_iter()
                        .map(|p| ProviderCandidate {
                            name: p.name().to_string(),
                            repo: p.db().map(|d| d.name().to_string()),
                            version: Some(p.version().to_string()),
                        })
                        .collect();
                    s.providers.push(ProviderPrompt { depend, candidates });
                    spq.set_index(0);
                }
                alpm::Question::ImportKey(mut iq) => {
                    iq.set_import(false);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("import-key; ");
                }
            }
        },
    );
    state
}

pub(crate) fn snapshot(state: &Rc<RefCell<RecorderState>>) -> QuestionSet {
    let s = state.borrow();
    QuestionSet {
        conflicts: s.conflicts.clone(),
        providers: s.providers.clone(),
        had_unsupported_question: s.had_unsupported,
        unsupported_summary: s.unsupported_summary.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::updates::compute_repo_upgrades;

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    const ROOT_DB_LCK: &str = "/var/lib/pacman/db.lck";
    const ROOT_SYNC_DIR: &str = "/var/lib/pacman/sync";

    fn sync_db_mtimes() -> BTreeMap<String, SystemTime> {
        let mut map = BTreeMap::new();
        let Ok(entries) = fs::read_dir(ROOT_SYNC_DIR) else {
            return map;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !(name.ends_with(".db") || name.ends_with(".files")) {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && let Ok(mtime) = meta.modified()
            {
                map.insert(name, mtime);
            }
        }
        map
    }

    #[test]
    #[ignore = "integration: needs live pacman sync DBs + user checkdb; run with --ignored rootless_sysupgrade"]
    fn rootless_sysupgrade_preview_matches_pacman_qu() {
        let root_lock = Path::new(ROOT_DB_LCK);
        assert!(
            !root_lock.exists(),
            "pre-existing {ROOT_DB_LCK} blocks a clean spike; remove it first"
        );
        let mtimes_before = sync_db_mtimes();

        let config = pacmanconf::Config::new().expect("failed to read pacman config");
        let checkdb_lock = crate::utils::cache_root()
            .expect("cache root")
            .join("checkdb")
            .join("db.lck");

        let mut handle = crate::pacman::init_alpm_rootless(&config)
            .expect("failed to build rootless alpm handle (Phase 1.1 primitive)");
        handle
            .syncdbs_mut()
            .update(false)
            .expect("failed to refresh checkdb sync DBs rootless");

        let manual: BTreeSet<String> = compute_repo_upgrades(&handle, &config)
            .expect("failed to compute manual repo upgrades")
            .iter()
            .map(|u| u.name.clone())
            .collect();

        crate::upgrade::apply_ignores(&mut handle, &config, &[]);
        handle
            .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
            .expect("failed to init sysupgrade dry-run transaction");
        handle
            .sync_sysupgrade(false)
            .expect("sync_sysupgrade failed to resolve upgrade targets");
        let spike: BTreeSet<String> = handle
            .trans_add()
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        let _ = handle.trans_release();

        assert!(
            !root_lock.exists(),
            "NOLOCK invariant violated: {ROOT_DB_LCK} was created"
        );
        assert!(
            !checkdb_lock.exists(),
            "NOLOCK invariant violated: checkdb db.lck was created"
        );
        assert_eq!(
            sync_db_mtimes(),
            mtimes_before,
            "no-write invariant violated: live sync DB mtimes changed"
        );

        let missing: Vec<String> = manual.difference(&spike).cloned().collect();
        assert!(
            missing.is_empty(),
            "every upgrade the manual vercmp sees (same checkdb data) must also be resolved by \
             rootless sync_sysupgrade. missing from spike: {missing:?}"
        );

        let preview =
            crate::dry_run::dry_sysupgrade(&mut handle).expect("dry_sysupgrade should succeed");
        assert!(
            !preview.summary.packages.is_empty(),
            "preview summary must list the direct upgrade set"
        );
        eprintln!(
            "[preview] upgrades={} conflicts={} providers={} prepare_error={:?}",
            preview.summary.packages.len(),
            preview.questions.conflicts.len(),
            preview.questions.providers.len(),
            preview.prepare_error,
        );
        for pkg in preview.summary.packages.iter().take(3) {
            eprintln!(
                "[preview] {} {} -> {}",
                pkg.name,
                pkg.old_version.as_deref().unwrap_or("-"),
                pkg.new_version,
            );
        }
    }
}
