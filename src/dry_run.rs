use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::question::{Conflict, QuestionSet};
use crate::resolve::BuildPlan;
use crate::stub_pkg::build_stub_pkg;

#[derive(Default)]
struct RecorderState {
    conflicts: Vec<Conflict>,
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

pub fn dry_run(handle: &mut alpm::Alpm, plan: &BuildPlan) -> anyhow::Result<QuestionSet> {
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
                    spq.set_index(0);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("select-provider; ");
                }
                alpm::Question::ImportKey(mut iq) => {
                    iq.set_import(false);
                    s.had_unsupported = true;
                    s.unsupported_summary.push_str("import-key; ");
                }
            }
        },
    );

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
    let outcome = match prepare_result {
        Ok(()) => Ok(snapshot),
        Err(err) => Err(classify_prepare_error(err)),
    };
    outcome
}

fn snapshot(state: &Rc<RefCell<RecorderState>>) -> QuestionSet {
    let s = state.borrow();
    QuestionSet {
        conflicts: s.conflicts.clone(),
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
