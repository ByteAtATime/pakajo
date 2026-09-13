use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
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

pub(crate) fn footer(
    transaction: Option<&Transaction>,
    updates: Element<'static>,
) -> Element<'static> {
    let badge = container(updates)
        .height(Length::Fixed(FOOTER_HEIGHT))
        .align_y(Vertical::Center);
    let Some(active) = transaction.filter(|t| !t.is_sysupgrade()) else {
        return container(badge)
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
    Row::new()
        .align_y(Vertical::Center)
        .width(Length::Fill)
        .push(container(badge).padding([6.0, 12.0]))
        .push(
            button::custom(cluster_row(label, bar))
                .padding([6.0, 12.0])
                .width(Length::Fill)
                .class(cosmic::theme::Button::Custom {
                    active: Box::new(|_, _| button::Style::new()),
                    disabled: Box::new(|_| button::Style::new()),
                    hovered: Box::new(|_, _| button::Style::new()),
                    pressed: Box::new(|_, _| button::Style::new()),
                })
                .on_press(crate::Message::OpenTransaction),
        )
        .into()
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
