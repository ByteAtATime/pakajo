use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::{Background, Border, Color, Length, Shadow, Vector};
use cosmic::widget::{Column, Row, button, container, space, text};
use pakajo::progress::InstallKind;
use pakajo::question::model::QuestionKey;
use pakajo::tx::convert::UnsatisfiedDep;

use super::TransactionMessage;
use super::shared::{destructive_color, mono_text, muted, muted_color, on_color};
use super::state::FailureKind;
use super::stepper::ghost_icon_style;
use crate::Element;
use crate::components::icons;

pub(crate) fn question_label(key: &QuestionKey) -> &'static str {
    match key {
        QuestionKey::Conflict { .. } => "Package conflict",
        QuestionKey::SelectProvider { .. } => "Provider choice",
        QuestionKey::Replace { .. } => "Package replacement",
        QuestionKey::InstallIgnorepkg { .. } => "Ignored package",
        QuestionKey::RemovePkgs { .. } => "Package removal",
        QuestionKey::HoldPkgs { .. } => "Held package",
        QuestionKey::Corrupted { .. } => "Corrupted package",
        QuestionKey::ImportKey { .. } => "Key import",
        QuestionKey::Proceed => "Proceed with transaction",
        QuestionKey::GroupMembers { .. } => "Group selection",
    }
}

pub(crate) fn failure_headline(kind: InstallKind, name: &str) -> String {
    let verb = match kind {
        InstallKind::Install => "install",
        InstallKind::Remove => "remove",
        InstallKind::Upgrade => "upgrade",
    };
    format!("Couldn't {verb} {name}")
}

pub(crate) fn dependents_sentence(cause: &str, count: usize) -> String {
    if count == 1 {
        format!("1 package still needs {cause}:")
    } else {
        format!("{count} packages still need {cause}:")
    }
}

pub(crate) fn unresolved_sentence(count: usize) -> String {
    if count == 1 {
        String::from("1 dependency couldn't be satisfied:")
    } else {
        format!("{count} dependencies couldn't be satisfied:")
    }
}

pub(crate) fn blocked_removal_sentence() -> &'static str {
    "Several packages couldn't be removed because others depend on them:"
}

pub(crate) fn failure_report(failure: &FailureKind) -> String {
    match failure {
        FailureKind::Prepare { reason, details } => {
            pakajo::tx::convert::PrepareFailure::new(reason.clone(), details.clone()).report(false)
        }
        FailureKind::Question { key, reason } => format!("{}: {reason}", question_label(key)),
        FailureKind::Message(message) => message.clone(),
    }
}

pub(crate) struct CauseGroup<'a> {
    pub(crate) cause: &'a str,
    pub(crate) targets: Vec<&'a UnsatisfiedDep>,
}

pub(crate) fn group_unsatisfied(
    details: &[UnsatisfiedDep],
) -> (Vec<CauseGroup<'_>>, Vec<&UnsatisfiedDep>) {
    let mut groups: Vec<CauseGroup<'_>> = Vec::new();
    let mut unresolved = Vec::new();
    for entry in details {
        match entry.cause.as_deref() {
            Some(cause) => match groups.iter_mut().find(|group| group.cause == cause) {
                Some(group) => group.targets.push(entry),
                None => groups.push(CauseGroup {
                    cause,
                    targets: vec![entry],
                }),
            },
            None => unresolved.push(entry),
        }
    }
    (groups, unresolved)
}

pub(crate) fn failure_card(
    name: &str,
    kind: InstallKind,
    failure: &FailureKind,
) -> Element<'static> {
    let mut sections = Column::new().spacing(16);
    for section in failure_sections(failure) {
        sections = sections.push(section);
    }
    let body = Column::new()
        .spacing(6)
        .push(failure_hero(name, kind, failure))
        .push(sections);
    container(body)
        .padding(24.0)
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| {
            let mut style = crate::components::theme::card_style(theme);
            style.border.radius = theme.cosmic().corner_radii.radius_m.into();
            style.border.color = Color {
                a: 0.5,
                ..destructive_color(theme)
            };
            style.shadow = Shadow {
                color: theme.cosmic().shade.into(),
                offset: Vector::new(0.0, 4.0),
                blur_radius: 16.0,
            };
            style
        })
        .into()
}

fn failure_hero(name: &str, kind: InstallKind, failure: &FailureKind) -> Element<'static> {
    let header = Row::new()
        .align_y(Vertical::Center)
        .spacing(12)
        .push(failure_icon())
        .push(text::title2(failure_headline(kind, name)))
        .push(space::horizontal())
        .push(copy_details_button());
    Column::new()
        .spacing(16)
        .push(header)
        .push(hero_sentence(failure))
        .into()
}

fn failure_icon() -> Element<'static> {
    let icon = cosmic::widget::icon(icons::circle_x())
        .size(22)
        .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
            move |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                color: Some(destructive_color(theme)),
            },
        )));
    container(icon)
        .width(40.0)
        .height(40.0)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.12,
                ..destructive_color(theme)
            })),
            border: Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn hero_sentence(failure: &FailureKind) -> Element<'static> {
    match failure {
        FailureKind::Prepare { reason, details } => prepare_sentence(reason, details),
        FailureKind::Question { key, reason } => Column::new()
            .spacing(4)
            .push(text(question_label(key).to_string()))
            .push(muted(text(reason.clone())))
            .into(),
        FailureKind::Message(message) => text(message.clone()).width(Length::Fill).into(),
    }
}

fn prepare_sentence(reason: &str, details: &[UnsatisfiedDep]) -> Element<'static> {
    let (groups, unresolved) = group_unsatisfied(details);
    if groups.is_empty() && unresolved.is_empty() {
        return muted(text(reason.to_string()));
    }
    let mut column = Column::new().spacing(4);
    if groups.len() == 1 {
        let group = &groups[0];
        column = column.push(text(dependents_sentence(group.cause, group.targets.len())));
    } else if !groups.is_empty() {
        column = column.push(text(blocked_removal_sentence().to_string()));
    }
    if !unresolved.is_empty() {
        column = column.push(text(unresolved_sentence(unresolved.len())));
    }
    column.into()
}

fn failure_sections(failure: &FailureKind) -> Vec<Element<'static>> {
    match failure {
        FailureKind::Prepare { details, .. } => prepare_sections(details),
        FailureKind::Question { .. } | FailureKind::Message(_) => Vec::new(),
    }
}

fn prepare_sections(details: &[UnsatisfiedDep]) -> Vec<Element<'static>> {
    let (groups, unresolved) = group_unsatisfied(details);
    let show_headers = groups.len() > 1;
    let mut sections: Vec<Element<'static>> = groups
        .iter()
        .map(|group| caused_section(group, show_headers))
        .collect();
    if !unresolved.is_empty() {
        sections.push(unresolved_section(&unresolved));
    }
    sections
}

fn caused_section(group: &CauseGroup<'_>, show_header: bool) -> Element<'static> {
    let mut column = Column::new().spacing(4);
    if show_header {
        column = column.push(muted(text::caption(format!(
            "{} is needed by",
            group.cause
        ))));
    }
    let mut rows = Column::new().spacing(6);
    for entry in &group.targets {
        rows = rows.push(caused_row(entry));
    }
    column.push(rows).into()
}

fn caused_row(entry: &UnsatisfiedDep) -> Element<'static> {
    dependency_row(entry.target.clone(), format!("requires {}", entry.depend))
}

fn unresolved_section(entries: &[&UnsatisfiedDep]) -> Element<'static> {
    let mut rows = Column::new().spacing(6);
    for entry in entries {
        rows = rows.push(unresolved_row(entry));
    }
    rows.into()
}

fn unresolved_row(entry: &UnsatisfiedDep) -> Element<'static> {
    dependency_row(
        entry.depend.clone(),
        format!("required by {}", entry.target),
    )
}

fn dependency_row(left: String, right: String) -> Element<'static> {
    let glyph = cosmic::widget::icon(icons::package())
        .size(14)
        .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
            move |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                color: Some(muted_color(theme)),
            },
        )));
    let content = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(glyph)
        .push(mono_text(&left))
        .push(space::horizontal())
        .push(muted(text(right)));
    container(content)
        .padding([6.0, 12.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.06,
                ..on_color(theme)
            })),
            border: Border {
                radius: theme.cosmic().corner_radii.radius_s.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn copy_details_button() -> Element<'static> {
    let icon = cosmic::widget::icon(icons::copy())
        .size(16)
        .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
            move |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                color: Some(muted_color(theme)),
            },
        )));
    button::custom(icon)
        .padding(8.0)
        .class(cosmic::theme::Button::Custom {
            active: Box::new(|_, _| button::Style::new()),
            disabled: Box::new(|_| button::Style::new()),
            hovered: Box::new(|_, theme| ghost_icon_style(theme, false)),
            pressed: Box::new(|_, theme| ghost_icon_style(theme, true)),
        })
        .on_press(crate::Message::Transaction(
            TransactionMessage::CopyFailureReport,
        ))
        .into()
}
