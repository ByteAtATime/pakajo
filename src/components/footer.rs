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
        TransactionStatus::Done(ChildOutcome::Stopped { .. }) => format!("Stopped - {name}"),
        TransactionStatus::Done(ChildOutcome::Failed(_)) => format!("Failed - {name}"),
        TransactionStatus::Done(ChildOutcome::Dismissed) => "Canceled authentication".to_string(),
        TransactionStatus::Done(ChildOutcome::NotFound(message)) => message.clone(),
    }
}

fn tint(transaction: &Transaction) -> Tint {
    match transaction.status() {
        TransactionStatus::Checking | TransactionStatus::Running => theme::on_color,
        TransactionStatus::Done(ChildOutcome::Success) => theme::success_color,
        TransactionStatus::Done(ChildOutcome::Stopped { idle: true }) => theme::success_color,
        TransactionStatus::Done(ChildOutcome::Stopped { idle: false }) => theme::warning_color,
        TransactionStatus::Done(ChildOutcome::Failed(_)) => theme::destructive_color,
        TransactionStatus::Done(ChildOutcome::NotFound(_)) => theme::destructive_color,
        TransactionStatus::Done(ChildOutcome::Dismissed) => theme::warning_color,
    }
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
    let progress = active.overall_progress();
    let label = text(summary(active.kind(), active.name(), active.status()));
    let tint = tint(active);
    Row::new()
        .align_y(Vertical::Center)
        .width(Length::Fill)
        .push(container(badge).padding([6.0, 12.0]))
        .push(
            button::custom(cluster_row(label.into(), progress))
                .padding([6.0, 12.0])
                .width(Length::Fill)
                .class(cosmic::theme::Button::Custom {
                    active: theme::tinted_button(tint, 0.8),
                    hovered: theme::tinted_button(tint, 1.0),
                    pressed: theme::tinted_button(tint, 1.0),
                    disabled: theme::tinted_button_disabled(tint, 0.8),
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
