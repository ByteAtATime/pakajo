use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, container, space, text};
use pakajo::events::PackageOp;
use pakajo::transaction_state::{InstallPackage, InstallState};

use crate::Element;
use crate::components::icons::circle_check;

use super::accordion::Section;
use super::shared::{
    accent_color, counter_suffix, destructive_color, muted, on_color, pill, success_color, tinted,
    version_change,
};
use super::state::StageState;

const GROUP_GAP: f32 = 8.0;

pub(super) fn install_section(install: &InstallState, state: StageState) -> Section<'_> {
    let mut section = Section {
        label: "Install",
        state,
        content: None,
        header_suffix: None,
    };
    match state {
        StageState::Done if install.order.len() == 1 => {
            section.header_suffix = Some(install_single_suffix(install));
        }
        StageState::Active if !install.order.is_empty() => {
            section.content = Some(install_view(install, false));
            if install.order.len() > 1 {
                section.header_suffix = Some(install_counter_suffix(install, false));
            }
        }
        StageState::Done if !install.order.is_empty() => {
            section.content = Some(install_view(install, true));
            section.header_suffix = Some(install_counter_suffix(install, true));
        }
        _ => {}
    }
    section
}

pub(super) fn install_view(state: &InstallState, done: bool) -> Element<'_> {
    let (finished, active): (Vec<_>, Vec<_>) = state
        .order
        .iter()
        .filter_map(|name| state.packages.get(name).map(|pkg| (name, pkg)))
        .partition(|(_, pkg)| done || pkg.completed);
    let show_gap = !finished.is_empty() && !active.is_empty();
    let mut col = Column::new().spacing(8);
    for (name, pkg) in finished {
        col = col.push(completed_row(name, pkg));
    }
    if show_gap {
        col = col.push(space::vertical().height(Length::Fixed(GROUP_GAP)));
    }
    for (name, pkg) in active {
        col = col.push(package_row(name, pkg));
    }
    col.into()
}

fn install_counter_suffix(state: &InstallState, done: bool) -> Element<'_> {
    let total = state.order.len();
    let finished = if done {
        total
    } else {
        state.packages.values().filter(|pkg| pkg.completed).count()
    };
    counter_suffix(finished, total, "packages")
}

fn install_single_suffix(state: &InstallState) -> Element<'_> {
    let entry = state
        .order
        .first()
        .and_then(|name| state.packages.get(name).map(|pkg| (name, pkg)));
    let Some((name, pkg)) = entry else {
        return muted(text("1 package"));
    };
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(package_name(name))
        .push(muted(
            text(version_change(
                pkg.old_version.as_deref(),
                pkg.new_version.as_deref(),
            ))
            .font(cosmic::font::mono()),
        ))
        .push(op_pill(pkg.operation))
        .into()
}

fn package_row<'a>(name: &'a str, pkg: &'a InstallPackage) -> Element<'a> {
    let top = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(package_name(name))
        .push(muted(
            text(version_change(
                pkg.old_version.as_deref(),
                pkg.new_version.as_deref(),
            ))
            .font(cosmic::font::mono()),
        ))
        .push(space::horizontal())
        .push(op_pill(pkg.operation))
        .push(tinted(text(format!("{:.0}%", pkg.percent)), accent_color));
    let bar = progress_bar(0.0..=100.0, pkg.percent)
        .length(Length::Fill)
        .girth(6.0);
    Column::new().spacing(6).push(top).push(bar).into()
}

fn completed_row<'a>(name: &'a str, pkg: &'a InstallPackage) -> Element<'a> {
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(cosmic::widget::icon(circle_check()).size(14))
        .push(package_name(name))
        .push(muted(
            text(version_change(
                pkg.old_version.as_deref(),
                pkg.new_version.as_deref(),
            ))
            .font(cosmic::font::mono()),
        ))
        .push(space::horizontal())
        .push(op_pill(pkg.operation))
        .into()
}

fn package_name(name: &str) -> Element<'static> {
    container(text(name.to_string()).font(cosmic::font::mono()))
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(on_color(t)),
            ..Default::default()
        })
        .into()
}

fn op_pill(operation: PackageOp) -> Element<'static> {
    let (label, color_fn): (_, fn(&cosmic::Theme) -> Color) = match operation {
        PackageOp::Install => ("Install", success_color),
        PackageOp::Upgrade => ("Upgrade", accent_color),
        PackageOp::Reinstall => ("Reinstall", accent_color),
        PackageOp::Downgrade => ("Downgrade", accent_color),
        PackageOp::Remove => ("Remove", destructive_color),
    };
    pill(label, color_fn)
}
