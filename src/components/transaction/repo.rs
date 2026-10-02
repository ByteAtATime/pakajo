use super::aur::build_section;
use super::failure::failure_card;
use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::prepare_section;
use super::shared::{counter_suffix, download_view, percent};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view};
use crate::Element;
use pakajo::progress::{AurStage, InstallKind, RepoStage, RepoState};

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = match model.kind {
        InstallKind::Install if model.targets.len() > 1 => {
            format!("Installing {} packages", model.targets.len())
        }
        InstallKind::Install => format!("Installing {}", model.name),
        InstallKind::Remove => format!("Removing {}", model.name),
        InstallKind::Upgrade => format!("Upgrading {}", model.name),
    };
    let mut sections = Vec::new();
    for (i, stage) in model.stages.iter().enumerate() {
        let state = model.stage_state(i);
        let section = match *stage {
            RepoStage::Validate => continue,
            RepoStage::Resolve => {
                prepare_section(&model.repo_state, prepare_state(model)).with_toggle_index(1)
            }
            RepoStage::Download => download_section(&model.repo_state, state).with_toggle_index(i),
            RepoStage::Install => {
                install_section(&model.repo_state.install, state, model.kind).with_toggle_index(i)
            }
            RepoStage::Finalize => {
                finalize_section(&model.repo_state.finalize, state).with_toggle_index(i)
            }
        };
        let expanded = if *stage == RepoStage::Resolve {
            model.expanded.contains(&1)
        } else {
            model.expanded.contains(&i)
        };
        sections.push((section, expanded));
    }
    if model.is_sysupgrade() && !model.aur.build_order.is_empty() {
        let build_index = model.stages.len();
        let section = build_section(model, model.aur_stage_state(AurStage::Build))
            .with_toggle_index(build_index);
        sections.push((section, model.expanded.contains(&build_index)));
    }
    let finished = matches!(model.status, TransactionStatus::Done(_));
    let failure = model
        .failure()
        .map(|failure| failure_card(&model.name, model.kind, failure));
    sections_view(title, failure, sections, finished, model.is_sysupgrade())
}

fn prepare_state(model: &TransactionModel) -> StageState {
    let resolve_state = model.stage_state(0);
    let validate_state = model.stage_state(1);
    if validate_state == StageState::Done {
        return StageState::Done;
    }
    if resolve_state == StageState::Failed || validate_state == StageState::Failed {
        return StageState::Failed;
    }
    if resolve_state == StageState::Done {
        return StageState::Active;
    }
    resolve_state
}

pub(super) fn download_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Download", state);
    if repo.download.total == 0 {
        return section;
    }
    match state {
        StageState::Active => {
            section.content = Some(download_view(&repo.download));
            section.suffix = Some(counter_suffix(
                repo.download.done,
                repo.download.total,
                "packages",
            ));
            if repo.download.bytes_total.max(0) > 0 {
                section.progress = Some(percent(
                    repo.download.bytes_done.max(0),
                    repo.download.bytes_total.max(0),
                ) as f32);
            }
        }
        StageState::Done => {
            section.content = Some(download_view(&repo.download));
            section.summary = Some(counter_suffix(
                repo.download.done,
                repo.download.total,
                "packages",
            ));
        }
        _ => {}
    }
    section
}
