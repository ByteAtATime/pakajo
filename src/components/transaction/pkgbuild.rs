use cosmic::iced::Length;
use cosmic::widget::{Row, button, dialog, scrollable, text};

use super::TransactionMessage;
use super::diff::{diff_rows_column, parse_unified_diff};
use crate::Element;

#[derive(Clone, Debug)]
pub enum PkgbuildMessage {
    SelectTab(usize),
}

pub(crate) struct ReviewedDiff {
    pub(crate) name: String,
    pub(crate) is_new: bool,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) lines: Vec<super::diff::DiffLine>,
}

impl ReviewedDiff {
    pub(crate) fn parse(diff: &pakajo::pkgbuild::PkgbuildDiff) -> Self {
        Self {
            name: diff.name.clone(),
            is_new: diff.is_new,
            dir: diff.dir.clone(),
            lines: parse_unified_diff(&diff.diff),
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
        let mut tabs = Row::new().spacing(4);
        for (i, entry) in self.entries.iter().enumerate() {
            let label = if entry.is_new {
                format!("{} (new)", entry.name)
            } else {
                entry.name.clone()
            };
            let item = button::standard(label).on_press(crate::Message::Transaction(
                TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(i)),
            ));
            tabs = tabs.push(item);
        }

        let body = match self.entries.get(self.current) {
            Some(entry) => scrollable(diff_rows_column(&entry.lines, entry.is_new))
                .height(Length::Fixed(400.0)),
            None => scrollable(text("No PKGBUILD to review")).height(Length::Fixed(400.0)),
        };

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

        let mut review = dialog().title("Review PKGBUILD");
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
