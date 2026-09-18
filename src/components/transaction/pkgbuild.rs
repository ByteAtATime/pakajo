use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::widget::{Row, button, container, dialog, scrollable, text};

use super::TransactionMessage;
use super::diff::diff_rows_column;
use super::shared::{pill, success_color};
use crate::Element;
use pakajo::diff::{RenderedLine, parse_unified_diff, rendered_lines};

#[derive(Clone, Debug)]
pub enum PkgbuildMessage {
    SelectTab(usize),
}

pub(crate) struct ReviewedDiff {
    pub(crate) name: String,
    pub(crate) is_new: bool,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) rows: Vec<RenderedLine>,
}

impl ReviewedDiff {
    pub(crate) fn parse(diff: &pakajo::pkgbuild::PkgbuildDiff) -> Self {
        let rows = rendered_lines(
            &parse_unified_diff(&diff.diff),
            diff.is_new,
            cosmic::theme::is_dark(),
        );
        Self {
            name: diff.name.clone(),
            is_new: diff.is_new,
            dir: diff.dir.clone(),
            rows,
        }
    }

    pub(crate) fn parse_all(diffs: &[pakajo::pkgbuild::PkgbuildDiff]) -> Vec<Self> {
        diffs.iter().map(Self::parse).collect()
    }
}

pub(crate) struct PkgbuildModel {
    pub(super) entries: Vec<ReviewedDiff>,
    pub(super) current: usize,
}

impl PkgbuildModel {
    pub(crate) fn new(diffs: Vec<pakajo::pkgbuild::PkgbuildDiff>) -> Self {
        Self {
            entries: ReviewedDiff::parse_all(&diffs),
            current: 0,
        }
    }

    pub(crate) fn update(&mut self, message: PkgbuildMessage) {
        match message {
            PkgbuildMessage::SelectTab(i) => {
                if i < self.entries.len() {
                    self.current = i;
                }
            }
        }
    }

    pub(crate) fn view(&self) -> Element<'_> {
        let mut tabs = Row::new().spacing(4).align_y(Vertical::Center);
        for (i, entry) in self.entries.iter().enumerate() {
            let mut tab_label = Row::new()
                .align_y(Vertical::Center)
                .spacing(8)
                .push(text(entry.name.clone()));
            if entry.is_new {
                tab_label = tab_label.push(pill("new", success_color));
            }
            let item = button::custom(tab_label)
                .class(cosmic::theme::Button::Standard)
                .on_press(crate::Message::Transaction(TransactionMessage::Pkgbuild(
                    PkgbuildMessage::SelectTab(i),
                )));
            tabs = tabs.push(item);
        }

        let scroll = match self.entries.get(self.current) {
            Some(entry) => {
                scrollable(diff_rows_column(&entry.rows, entry.is_new)).height(Length::Fill)
            }
            None => scrollable(text("No PKGBUILD to review")).height(Length::Fill),
        };
        let body = container(scroll).width(Length::Fill).height(Length::Fill);

        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelPkgbuild,
        ));
        let primary = if self.current + 1 < self.entries.len() {
            button::suggested("Next").on_press(crate::Message::Transaction(
                TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(self.current + 1)),
            ))
        } else {
            button::suggested("Accept").on_press(crate::Message::Transaction(
                TransactionMessage::ApprovePkgbuild,
            ))
        };

        let mut review = dialog()
            .title("Review PKGBUILD")
            .width(Length::Fill)
            .max_width(1100.0)
            .height(Length::Fill)
            .max_height(800.0);
        if self.entries.len() > 1 {
            review = review.control(tabs);
        }
        review
            .control(body)
            .primary_action(primary)
            .secondary_action(cancel)
            .into()
    }
}
