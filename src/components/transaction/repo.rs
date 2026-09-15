use super::aur::{build_section, failure_note};
use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::prepare_section;
use super::shared::{counter_suffix, download_view, percent};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view};
use crate::Element;
use cosmic::widget::Column;
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
        if *stage == RepoStage::Validate {
            continue;
        }
        let mut section = if *stage == RepoStage::Resolve {
            prepare_section(&model.repo_state, prepare_state(model)).with_toggle_index(1)
        } else {
            let state = model.stage_state(i);
            let section = match *stage {
                RepoStage::Resolve => prepare_section(&model.repo_state, state),
                RepoStage::Validate => continue,
                RepoStage::Download => download_section(&model.repo_state, state),
                RepoStage::Install => install_section(&model.repo_state.install, state, model.kind),
                RepoStage::Finalize => finalize_section(&model.repo_state.finalize, state),
            };
            section.with_toggle_index(i)
        };
        if section.state == StageState::Failed
            && !model.build_owns_failure()
            && let Some(message) = model.failure_message.as_deref()
        {
            let note = failure_note(message);
            section.content = Some(match section.content {
                Some(existing) => Column::new().spacing(10).push(note).push(existing).into(),
                None => note,
            });
        }
        let expanded = if *stage == RepoStage::Resolve {
            model.expanded.contains(&1)
        } else {
            model.expanded.contains(&i)
        };
        sections.push((section, expanded));
    }
    if model.is_sysupgrade() && !model.aur.build_order.is_empty() {
        let build_index = model.stages.len();
        let mut section = build_section(model, model.aur_stage_state(AurStage::Build))
            .with_toggle_index(build_index);
        if section.state == StageState::Failed
            && let Some(message) = model.failure_message.as_deref()
        {
            let note = failure_note(message);
            section.content = Some(match section.content {
                Some(existing) => Column::new().spacing(10).push(note).push(existing).into(),
                None => note,
            });
        }
        sections.push((section, model.expanded.contains(&build_index)));
    }
    let finished = matches!(model.status, TransactionStatus::Done(_));
    sections_view(title, sections, finished, model.is_sysupgrade())
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
