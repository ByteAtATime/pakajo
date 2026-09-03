use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::events::TransactionSummary;
use crate::question::{Conflict, ProviderCandidate, ProviderPrompt, QuestionSet};
use crate::resolve::BuildPlan;
use crate::stub_pkg::build_stub_pkg;

#[derive(Default)]
struct RecorderState {
    conflicts: Vec<Conflict>,
    providers: Vec<ProviderPrompt>,
    had_unsupported: bool,
    unsupported_summary: String,
}

pub fn dry_run_for_target(target: &str) -> anyhow::Result<QuestionSet> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut alpm = crate::pacman::init_alpm(&config)?;
    let aur = crate::aur::AurClient::new();
    let plan = crate::resolve::resolve(
        &crate::resolve::AlpmDb(&alpm),
        &aur,
        &[target.to_string()],
        false,
    )?;
    dry_run(&mut alpm, &plan)
}

pub fn dry_run_for_repo_targets(targets: &[String]) -> anyhow::Result<QuestionSet> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut alpm = crate::pacman::init_alpm(&config)?;
    repo_dry_run(&mut alpm, targets)
}

pub fn repo_dry_run(handle: &mut alpm::Alpm, targets: &[String]) -> anyhow::Result<QuestionSet> {
    let state = attach_recorder(handle);
    let outcome = run_repo_dry_run_transaction(handle, targets, &state);
    let _ = handle.trans_release();
    outcome
}

#[derive(Debug, Clone)]
pub struct SysupgradePreview {
    pub summary: TransactionSummary,
    pub questions: QuestionSet,
    pub prepare_error: Option<PrepareFailure>,
    pub aur: Vec<crate::upgrade::AurUpgradeCandidate>,
    pub pkgbuild_diffs: Vec<crate::pkgbuild::PkgbuildDiff>,
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

pub fn compute_sysupgrade_preview(
    handle: &mut alpm::Alpm,
    config: &pacmanconf::Config,
) -> anyhow::Result<SysupgradePreview> {
    let aur_client = crate::aur::AurClient::new();
    let aur = match crate::upgrade::compute_aur_upgrades(handle, &aur_client) {
        Ok((v, _)) => v,
        Err(e) => {
            eprintln!(
                "[pakajo] aur upgrade check failed, sysupgrade preview shows repo only: {e:#}"
            );
            Vec::new()
        }
    };
    crate::upgrade::apply_ignores(handle, config, &[]);
    let state = attach_recorder(handle);
    let mut preview = run_sysupgrade_preview(handle, &state);
    let _ = handle.trans_release();
    if let Ok(p) = &mut preview {
        p.aur = aur;
    }
    preview
}

fn run_sysupgrade_preview(
    handle: &mut alpm::Alpm,
    state: &Rc<RefCell<RecorderState>>,
) -> anyhow::Result<SysupgradePreview> {
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init sysupgrade preview transaction")?;
    handle
        .sync_sysupgrade(false)
        .context("sync_sysupgrade failed to resolve upgrade targets")?;
    let prepare_error = handle.trans_prepare().err().map(extract_prepare_failure);
    let questions = snapshot(state);
    let summary = crate::install::build_summary(handle);
    Ok(SysupgradePreview {
        summary,
        questions,
        prepare_error,
        aur: Vec::new(),
        pkgbuild_diffs: Vec::new(),
    })
}

pub fn default_repo_summary(
    handle: &mut alpm::Alpm,
) -> anyhow::Result<crate::events::TransactionSummary> {
    let state = attach_recorder(handle);
    let preview = run_sysupgrade_preview(handle, &state);
    let _ = handle.trans_release();
    preview.map(|p| p.summary)
}

fn extract_prepare_failure(err: alpm::PrepareError) -> PrepareFailure {
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

fn attach_recorder(handle: &mut alpm::Alpm) -> Rc<RefCell<RecorderState>> {
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

pub fn dry_run(handle: &mut alpm::Alpm, plan: &BuildPlan) -> anyhow::Result<QuestionSet> {
    let state = attach_recorder(handle);
    let outcome = run_dry_run_transaction(handle, plan, &state);
    let _ = handle.trans_release();
    outcome
}

fn run_dry_run_transaction(
    handle: &mut alpm::Alpm,
    plan: &BuildPlan,
    state: &Rc<RefCell<RecorderState>>,
) -> anyhow::Result<QuestionSet> {
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init dry-run transaction")?;

    let stub_dir = tempfile::tempdir().context("failed to create stub work dir")?;
    for layer in &plan.layers {
        for info in &layer.aur {
            let path = build_stub_pkg(info, stub_dir.path())
                .with_context(|| format!("failed to build stub for {}", info.name))?;
            let loaded = handle
                .pkg_load(path.to_string_lossy().as_ref(), false, alpm::SigLevel::NONE)
                .with_context(|| format!("failed to load stub for {}", info.name))?;
            handle
                .trans_add_pkg(loaded)
                .map_err(alpm::Error::from)
                .with_context(|| format!("failed to queue stub for {}", info.name))?;
        }
    }

    let prepare_result = handle.trans_prepare();
    let snapshot = snapshot(state);

    match prepare_result {
        Ok(()) => Ok(snapshot),
        Err(err) => Err(classify_prepare_error(err)),
    }
}

fn run_repo_dry_run_transaction(
    handle: &mut alpm::Alpm,
    targets: &[String],
    state: &Rc<RefCell<RecorderState>>,
) -> anyhow::Result<QuestionSet> {
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init dry-run transaction")?;
    for target in targets {
        let pkg = crate::pacman::find_pkg(handle, target)
            .ok_or_else(|| anyhow::anyhow!("package '{target}' not found in any repository"))?;
        handle
            .trans_add_pkg(pkg)
            .map_err(alpm::Error::from)
            .with_context(|| format!("failed to queue package for dry-run: {target}"))?;
    }
    let prepare_result = handle.trans_prepare();
    let snapshot = snapshot(state);
    match prepare_result {
        Ok(()) => Ok(snapshot),
        Err(err) => Err(classify_prepare_error(err)),
    }
}

fn snapshot(state: &Rc<RefCell<RecorderState>>) -> QuestionSet {
    let s = state.borrow();
    QuestionSet {
        conflicts: s.conflicts.clone(),
        providers: s.providers.clone(),
        had_unsupported_question: s.had_unsupported,
        unsupported_summary: s.unsupported_summary.clone(),
    }
}

fn classify_prepare_error(err: alpm::PrepareError) -> anyhow::Error {
    match err.data() {
        Some(alpm::PrepareData::ConflictingDeps(list)) => {
            let detail = list
                .iter()
                .map(|c| format!("{} vs {}", c.package1().name(), c.package2().name()))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("unresolvable conflict(s): {detail}")
        }
        Some(other) => anyhow::anyhow!("dry_run trans_prepare failed: {other:?}"),
        None => anyhow::anyhow!("dry_run trans_prepare failed: {}", err.error()),
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
        let checkdb_lock = crate::build::cache_root()
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

        let preview = crate::dry_run::compute_sysupgrade_preview(&mut handle, &config)
            .expect("compute_sysupgrade_preview should succeed");
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
