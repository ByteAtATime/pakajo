use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::prepare_section;
use super::shared::{counter_suffix, download_view};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view};
use crate::Element;
use pakajo::progress::{InstallKind, RepoStage, RepoState};

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = match model.kind {
        InstallKind::Install => format!("Installing {}", model.name),
        InstallKind::Remove => format!("Removing {}", model.name),
        InstallKind::Upgrade => format!("Upgrading {}", model.name),
    };
    let mut sections = Vec::new();
    for (i, stage) in model.stages.iter().enumerate() {
        if *stage == RepoStage::Validate {
            continue;
        }
        let section = if *stage == RepoStage::Resolve {
            prepare_section(&model.repo_state, prepare_state(model)).with_toggle_index(1)
        } else {
            let state = model.stage_state(i);
            let section = match *stage {
                RepoStage::Resolve => prepare_section(&model.repo_state, state),
                RepoStage::Validate => continue,
                RepoStage::Download => download_section(&model.repo_state, state),
                RepoStage::Install => install_section(&model.repo_state.install, state),
                RepoStage::Finalize => finalize_section(&model.repo_state.finalize, state),
            };
            section.with_toggle_index(i)
        };
        let expanded = if *stage == RepoStage::Resolve {
            model.expanded.contains(&1)
        } else {
            model.expanded.contains(&i)
        };
        sections.push((section, expanded));
    }
    let finished = matches!(model.status, TransactionStatus::Done(_));
    sections_view(title, sections, finished)
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
    if matches!(state, StageState::Active | StageState::Done) {
        section.content = Some(download_view(&repo.download));
        section.header_suffix = Some(counter_suffix(
            repo.download.done,
            repo.download.total,
            "packages",
        ));
    }
    section
}
