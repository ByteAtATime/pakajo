use cosmic::iced::Background;
use cosmic::iced::Color;
use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::border::Radius;
use cosmic::iced::widget::progress_bar;
use cosmic::widget::{Row, button, container, space, text};

use pakajo::dispatch::exec::ChildOutcome;
use pakajo::progress::InstallKind;

use crate::Element;
use crate::components::theme;
use crate::components::transaction::{Transaction, TransactionStatus};

const FOOTER_HEIGHT: f32 = 36.0;
const BAR_WIDTH: f32 = 140.0;

type Tint = fn(&cosmic::Theme) -> cosmic::iced::Color;

fn summary(kind: InstallKind, name: &str, status: &TransactionStatus) -> String {
    let (active_verb, done_verb) = match kind {
        InstallKind::Install => ("Installing", "Installed"),
        InstallKind::Remove => ("Removing", "Removed"),
        InstallKind::Upgrade => ("Upgrading", "Upgraded"),
    };
    match status {
        TransactionStatus::Checking | TransactionStatus::Running => {
            format!("{active_verb} {name}...")
        }
        TransactionStatus::Done(ChildOutcome::Success) => format!("{done_verb} {name}"),
        TransactionStatus::Done(ChildOutcome::Failed(_)) => format!("Failed - {name}"),
        TransactionStatus::Done(ChildOutcome::Dismissed) => "Canceled authentication".to_string(),
        TransactionStatus::Done(ChildOutcome::NotFound) => format!("Not found - {name}"),
    }
}

fn bar_and_tint(transaction: &Transaction) -> (f32, Tint) {
    let value = transaction.overall_progress();
    let tint = match transaction.status() {
        TransactionStatus::Checking | TransactionStatus::Running => theme::success_color,
        TransactionStatus::Done(ChildOutcome::Success) => theme::success_color,
        TransactionStatus::Done(ChildOutcome::Failed(_)) => theme::destructive_color,
        TransactionStatus::Done(ChildOutcome::NotFound) => theme::destructive_color,
        TransactionStatus::Done(ChildOutcome::Dismissed) => theme::warning_color,
    };
    (value, tint)
}

pub(crate) fn footer(transaction: Option<&Transaction>) -> Element<'static> {
    let Some(active) = transaction.filter(|t| !t.is_sysupgrade()) else {
        return container(space::horizontal().height(Length::Fixed(FOOTER_HEIGHT)))
            .padding([6.0, 12.0])
            .width(Length::Fill)
            .into();
    };
    let (bar, tint) = bar_and_tint(active);
    let label = text(summary(active.kind(), active.name(), active.status()));
    let label = match active.status() {
        TransactionStatus::Done(_) => theme::tinted(label, tint),
        TransactionStatus::Checking | TransactionStatus::Running => label.into(),
    };
    button::custom(cluster_row(label, bar))
        .padding([6.0, 12.0])
        .width(Length::Fill)
        .class(cosmic::theme::Button::Custom {
            active: Box::new(|_, _| button::Style::new()),
            disabled: Box::new(|_| button::Style::new()),
            hovered: Box::new(|_, theme| footer_button_style(theme, false)),
            pressed: Box::new(|_, theme| footer_button_style(theme, true)),
        })
        .on_press(crate::Message::OpenTransaction)
        .into()
}

fn footer_button_style(theme: &cosmic::Theme, pressed: bool) -> button::Style {
    let component = &theme.cosmic().background(false).component;
    let tint = if pressed {
        component.pressed
    } else {
        component.hover
    };
    button::Style {
        background: Some(Background::Color(Color::from(tint))),
        border_radius: Radius::from(0.0),
        ..button::Style::new()
    }
}

fn cluster_row(label: Element<'static>, bar: f32) -> Element<'static> {
    Row::new()
        .align_y(Vertical::Center)
        .height(Length::Fixed(FOOTER_HEIGHT))
        .push(space::horizontal())
        .push(
            Row::new()
                .spacing(8)
                .align_y(Vertical::Center)
                .push(label)
                .push(
                    progress_bar(0.0..=100.0, bar)
                        .length(Length::Fixed(BAR_WIDTH))
                        .girth(6.0),
                ),
        )
        .width(Length::Fill)
        .into()
}
