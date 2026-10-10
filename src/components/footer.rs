use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::widget::{Row, button, container, icon, space, text};

use pakajo::dispatch::exec::ChildOutcome;
use pakajo::progress::InstallKind;

use crate::Element;
use crate::components::icons;
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
        TransactionStatus::Done(ChildOutcome::Cancelled) => format!("Cancelled - {name}"),
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
        TransactionStatus::Done(ChildOutcome::Cancelled) => theme::warning_color,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trailing {
    Bar,
    Check,
    Alert,
    Cross,
}

fn trailing(status: &TransactionStatus) -> Trailing {
    match status {
        TransactionStatus::Checking | TransactionStatus::Running => Trailing::Bar,
        TransactionStatus::Done(ChildOutcome::Success) => Trailing::Check,
        TransactionStatus::Done(ChildOutcome::Stopped { idle: true }) => Trailing::Check,
        TransactionStatus::Done(ChildOutcome::Failed(_)) => Trailing::Cross,
        TransactionStatus::Done(ChildOutcome::NotFound(_)) => Trailing::Cross,
        TransactionStatus::Done(ChildOutcome::Stopped { idle: false }) => Trailing::Alert,
        TransactionStatus::Done(ChildOutcome::Dismissed) => Trailing::Alert,
        TransactionStatus::Done(ChildOutcome::Cancelled) => Trailing::Alert,
    }
}

fn trailing_icon(trailing: Trailing, tint: Tint) -> Element<'static> {
    let handle = match trailing {
        Trailing::Bar => icons::circle_dot(),
        Trailing::Check => icons::circle_check(),
        Trailing::Alert => icons::triangle_alert(),
        Trailing::Cross => icons::circle_x(),
    };
    container(
        icon(handle)
            .size(16)
            .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                move |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                    color: Some(tint(theme)),
                },
            ))),
    )
    .align_y(Vertical::Center)
    .into()
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
    let end = match trailing(active.status()) {
        Trailing::Bar => progress_bar(0.0..=100.0, progress)
            .length(Length::Fixed(BAR_WIDTH))
            .girth(6.0)
            .into(),
        kind => trailing_icon(kind, tint),
    };
    Row::new()
        .align_y(Vertical::Center)
        .width(Length::Fill)
        .push(container(badge).padding([6.0, 12.0]))
        .push(
            button::custom(cluster_row(label.into(), end))
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

fn cluster_row(label: Element<'static>, end: Element<'static>) -> Element<'static> {
    Row::new()
        .align_y(Vertical::Center)
        .height(Length::Fixed(FOOTER_HEIGHT))
        .push(space::horizontal())
        .push(
            Row::new()
                .spacing(8)
                .align_y(Vertical::Center)
                .push(label)
                .push(end),
        )
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::Trailing;
    use super::trailing;
    use crate::components::transaction::TransactionStatus;
    use pakajo::dispatch::exec::ChildOutcome;

    #[test]
    fn active_transactions_keep_progress_bar() {
        assert_eq!(trailing(&TransactionStatus::Checking), Trailing::Bar);
        assert_eq!(trailing(&TransactionStatus::Running), Trailing::Bar);
    }

    #[test]
    fn successful_terminal_states_show_check() {
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Success)),
            Trailing::Check
        );
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Stopped {
                idle: true
            })),
            Trailing::Check
        );
    }

    #[test]
    fn failed_terminal_states_show_cross() {
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Failed(
                String::from("boom")
            ))),
            Trailing::Cross
        );
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::NotFound(
                String::from("missing")
            ))),
            Trailing::Cross
        );
    }

    #[test]
    fn cancelled_terminal_states_show_alert() {
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Cancelled)),
            Trailing::Alert
        );
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Dismissed)),
            Trailing::Alert
        );
        assert_eq!(
            trailing(&TransactionStatus::Done(ChildOutcome::Stopped {
                idle: false
            })),
            Trailing::Alert
        );
    }
}
