use cosmic::iced::Length;
use cosmic::widget::{Column, Row, button, container, scrollable, space, text};

use pakajo::pkgbuild::PkgbuildDiff;

use super::TransactionMessage;

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

    pub(crate) fn view(&self) -> cosmic::Element<'_, crate::Message> {
        let mut tabs = Row::new().spacing(4);
        for (i, diff) in self.diffs.iter().enumerate() {
            let label = if diff.is_new {
                format!("{} (new)", diff.name)
            } else {
                diff.name.clone()
            };
            let item = button::custom(text(label)).on_press(crate::Message::Transaction(
                TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(i)),
            ));
            tabs = tabs.push(item);
        }

        let body = match self.diffs.get(self.current) {
            Some(diff) => {
                let mut lines = Column::new().spacing(0);
                for line in diff.diff.lines() {
                    lines = lines.push(text(line.to_string()).font(cosmic::font::mono()));
                }
                scrollable(lines).height(Length::Fixed(400.0))
            }
            None => scrollable(text("No PKGBUILD to review")).height(Length::Fixed(400.0)),
        };

        let col = Column::new()
            .spacing(16)
            .push(text("Review PKGBUILD"))
            .push_maybe((self.diffs.len() > 1).then(|| tabs))
            .push(body)
            .push(pkgbuild_footer(self.current, self.diffs.len()));

        container(col)
            .class(cosmic::theme::Container::Dialog(true))
            .padding([24.0, 24.0])
            .width(Length::Fixed(570.0))
            .into()
    }
}

fn pkgbuild_footer(current: usize, len: usize) -> cosmic::Element<'static, crate::Message> {
    let cancel = button::custom(text("Cancel")).on_press(crate::Message::Transaction(
        TransactionMessage::CancelPkgbuild,
    ));
    let right = if current + 1 < len {
        button::custom(text("Next")).on_press(crate::Message::Transaction(
            TransactionMessage::Pkgbuild(PkgbuildMessage::SelectTab(current + 1)),
        ))
    } else {
        button::custom(text("Accept")).on_press(crate::Message::Transaction(
            TransactionMessage::ApprovePkgbuild,
        ))
    };
    Row::new()
        .spacing(8)
        .push(space::horizontal())
        .push(cancel)
        .push(right)
        .into()
}
