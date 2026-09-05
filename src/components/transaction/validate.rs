use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::widget::{Row, text};
use pakajo::progress::{RepoState, VALIDATE_TOTAL};

use crate::Element;

use super::accordion::Section;
use super::shared::muted;
use super::state::StageState;

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
