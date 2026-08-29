use cosmic::iced::{Color, Length};
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
            Some(diff) => scrollable(diff_lines_column(diff)).height(Length::Fixed(400.0)),
            None => scrollable(text("No PKGBUILD to review")).height(Length::Fixed(400.0)),
        };

        let col = Column::new()
            .spacing(16)
            .push(text("Review PKGBUILD"))
            .push_maybe((self.diffs.len() > 1).then_some(tabs))
            .push(body)
            .push(pkgbuild_footer(self.current, self.diffs.len()));

        container(col)
            .class(cosmic::theme::Container::Dialog(true))
            .padding([24.0, 24.0])
            .width(Length::Fixed(570.0))
            .into()
    }
}

#[derive(Clone, Copy)]
enum DiffTone {
    Muted,
    Added,
    Removed,
}

pub(crate) fn diff_lines_column(diff: &PkgbuildDiff) -> cosmic::Element<'_, crate::Message> {
    let mut lines = Column::new().spacing(0);
    for line in diff.diff.lines() {
        let line_widget = text(line.to_string()).font(cosmic::font::mono());
        let element: cosmic::Element<'_, crate::Message> = match diff_tone(line) {
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

fn muted_color(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    Color { a: 0.5, ..on }
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
