use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::widget::{Row, text};
use pakajo::progress::{InstallKind, RepoStage, RepoState, VALIDATE_TOTAL};

use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::resolve_section;
use super::shared::{counter_suffix, download_view, muted};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view};
use crate::Element;

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = match model.kind {
        InstallKind::Install => format!("Installing {}", model.name),
        InstallKind::Remove => format!("Removing {}", model.name),
        InstallKind::Upgrade => format!("Upgrading {}", model.name),
    };
    let sections = model
        .stages
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let state = model.stage_state(i);
            let section = match *stage {
                RepoStage::Resolve => resolve_section(&model.repo_state, state),
                RepoStage::Validate => validate_section(&model.repo_state, state),
                RepoStage::Download => download_section(&model.repo_state, state),
                RepoStage::Install => install_section(&model.repo_state.install, state),
                RepoStage::Finalize => finalize_section(&model.repo_state.finalize, state),
            };
            (section, model.expanded.contains(&i))
        })
        .collect();
    let finished = matches!(model.status, TransactionStatus::Done(_));
    sections_view(title, sections, finished)
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

pub(super) fn validate_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Validate", state);
    section.header_suffix = match state {
        StageState::Active => Some(validate_suffix(repo, false)),
        StageState::Done => Some(validate_suffix(repo, true)),
        _ => None,
    };
    section
}

fn validate_suffix(repo: &RepoState, done: bool) -> Element<'_> {
    let total = VALIDATE_TOTAL;
    let count = if done { total } else { repo.validate.count() };
    let step = if done {
        "Validated"
    } else {
        repo.validate.label
    };
    let bar = progress_bar(0.0..=100.0, count as f32 / total as f32 * 100.0)
        .length(Length::Fixed(120.0))
        .girth(6.0);
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(format!("{step} ({count}/{total})"))))
        .push(bar)
        .into()
}
