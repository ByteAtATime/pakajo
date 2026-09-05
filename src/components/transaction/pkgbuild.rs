use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, button, container, dialog, scrollable, text};

use pakajo::pkgbuild::PkgbuildDiff;

use super::TransactionMessage;
use super::shared::muted_color;
use crate::Element;

#[derive(Clone, Debug)]
pub enum PkgbuildMessage {
    SelectTab(usize),
}

pub(crate) struct PkgbuildModel {
    pub(super) diffs: Vec<PkgbuildDiff>,
    pub(super) current: usize,
}

impl PkgbuildModel {
    pub(crate) fn new(diffs: Vec<PkgbuildDiff>) -> Self {
        Self { diffs, current: 0 }
    }

    pub(crate) fn update(&mut self, message: PkgbuildMessage) {
        match message {
            PkgbuildMessage::SelectTab(i) => {
                if i < self.diffs.len() {
                    self.current = i;
                }
            }
        }
    }

    pub(crate) fn view(&self) -> Element<'_> {
        let mut tabs = Row::new().spacing(4);
        for (i, diff) in self.diffs.iter().enumerate() {
            let label = if diff.is_new {
                format!("{} (new)", diff.name)
            } else {
                diff.name.clone()
            };
            let item = button::standard(label).on_press(crate::Message::Transaction(
                TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(i)),
            ));
            tabs = tabs.push(item);
        }

        let body = match self.diffs.get(self.current) {
            Some(diff) => scrollable(diff_lines_column(diff)).height(Length::Fixed(400.0)),
            None => scrollable(text("No PKGBUILD to review")).height(Length::Fixed(400.0)),
        };

        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelPkgbuild,
        ));
        let primary = if self.current + 1 < self.diffs.len() {
            button::suggested("Next").on_press(crate::Message::Transaction(
                TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(self.current + 1)),
            ))
        } else {
            button::suggested("Accept").on_press(crate::Message::Transaction(
                TransactionMessage::ApprovePkgbuild,
            ))
        };

        let mut review = dialog().title("Review PKGBUILD");
        if self.diffs.len() > 1 {
            review = review.control(tabs);
        }
        review
            .control(body)
            .primary_action(primary)
            .secondary_action(cancel)
            .into()
    }
}

#[derive(Clone, Copy)]
enum DiffTone {
    Muted,
    Added,
    Removed,
}

pub(crate) fn diff_lines_column(diff: &PkgbuildDiff) -> Element<'_> {
    let mut lines = Column::new().spacing(0);
    for line in diff.diff.lines() {
        let line_widget = text::monotext(line.to_string());
        let element: Element<'_> = match diff_tone(line) {
            Some(tone) => container(line_widget)
                .style(move |theme: &cosmic::Theme| tone_style(theme, tone))
                .into(),
            None => line_widget.into(),
        };
        lines = lines.push(element);
    }
    lines.into()
}

fn diff_tone(line: &str) -> Option<DiffTone> {
    if line.starts_with("@@") {
        Some(DiffTone::Muted)
    } else if line.starts_with("+") {
        Some(DiffTone::Added)
    } else if line.starts_with("-") {
        Some(DiffTone::Removed)
    } else {
        None
    }
}

fn tone_style(theme: &cosmic::Theme, tone: DiffTone) -> container::Style {
    let color = match tone {
        DiffTone::Muted => muted_color(theme),
        DiffTone::Added => Color::from(theme.cosmic().success.base),
        DiffTone::Removed => Color::from(theme.cosmic().destructive.base),
    };
    container::Style {
        text_color: Some(color),
        ..Default::default()
    }
}
