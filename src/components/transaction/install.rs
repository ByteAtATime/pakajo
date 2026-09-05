use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, space, text};
use pakajo::events::PackageOp;
use pakajo::transaction_state::{InstallPackage, InstallState};

use crate::Element;
use crate::components::icons::circle_check;

use super::accordion::Section;
use super::shared::{
    accent_color, counter_suffix, destructive_color, mono_text, muted, pill, success_color,
    thin_bar, tinted, version_change,
};
use super::state::StageState;

const GROUP_GAP: f32 = 8.0;

pub(super) fn install_section(install: &InstallState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Install", state);
    match state {
        StageState::Done if install.order.len() == 1 => {
            section.header_suffix = Some(install_single_suffix(install));
        }
        StageState::Active | StageState::Done if !install.order.is_empty() => {
            let done = state == StageState::Done;
            section.content = Some(install_view(install, done));
            if install.order.len() > 1 {
                section.header_suffix = Some(install_counter_suffix(install, done));
            }
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
        .push(mono_text(name))
        .push(version_label(pkg))
        .push(op_pill(pkg.operation))
        .into()
}

fn version_label(pkg: &InstallPackage) -> Element<'static> {
    muted(
        text(version_change(
            pkg.old_version.as_deref(),
            pkg.new_version.as_deref(),
        ))
        .font(cosmic::font::mono()),
    )
}

fn package_row<'a>(name: &'a str, pkg: &'a InstallPackage) -> Element<'a> {
    let top = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(mono_text(name))
        .push(version_label(pkg))
        .push(space::horizontal())
        .push(op_pill(pkg.operation))
        .push(tinted(text(format!("{:.0}%", pkg.percent)), accent_color));
    Column::new()
        .spacing(6)
        .push(top)
        .push(thin_bar(pkg.percent))
        .into()
}

fn completed_row<'a>(name: &'a str, pkg: &'a InstallPackage) -> Element<'a> {
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(cosmic::widget::icon(circle_check()).size(14))
        .push(mono_text(name))
        .push(version_label(pkg))
        .push(space::horizontal())
        .push(op_pill(pkg.operation))
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
