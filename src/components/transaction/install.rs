use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, space, text};
use pakajo::events::PackageOp;
use pakajo::progress::{InstallKind, InstallPackage, InstallState};

use crate::Element;
use crate::components::icons::circle_check;

use super::shared::{
    accent_color, counter_suffix, destructive_color, mono_text, muted, percent, pill,
    single_summary, success_color, thin_bar, tinted, version_change,
};
use super::state::StageState;
use super::stepper::Section;

const GROUP_GAP: f32 = 8.0;

pub(super) fn install_section(
    install: &InstallState,
    state: StageState,
    kind: InstallKind,
) -> Section<'_> {
    let label = match kind {
        InstallKind::Remove => "Remove",
        InstallKind::Install | InstallKind::Upgrade => "Install",
    };
    let mut section = Section::new(label, state);
    match state {
        StageState::Done if install.order.len() == 1 => {
            section.summary = Some(install_single_suffix(install));
        }
        StageState::Active if !install.order.is_empty() => {
            section.content = Some(install_view(install, false));
            section.suffix = Some(install_counter_suffix(install));
            section.progress = Some(install_percent(install));
        }
        StageState::Done if !install.order.is_empty() => {
            section.content = Some(install_view(install, true));
            if install.order.len() > 1 {
                section.summary = Some(counter_suffix(
                    install.order.len(),
                    install.order.len(),
                    "packages",
                ));
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

fn install_finished(state: &InstallState) -> usize {
    state.packages.values().filter(|pkg| pkg.completed).count()
}

fn install_counter_suffix(state: &InstallState) -> Element<'static> {
    counter_suffix(install_finished(state), state.order.len(), "packages")
}

fn install_percent(state: &InstallState) -> f32 {
    percent(install_finished(state) as i64, state.order.len() as i64) as f32
}

fn install_single_suffix(state: &InstallState) -> Element<'_> {
    let entry = state
        .order
        .first()
        .and_then(|name| state.packages.get(name).map(|pkg| (name, pkg)));
    let Some((name, pkg)) = entry else {
        return muted(text("1 package"));
    };
    let version = version_change(pkg.old_version.as_deref(), pkg.new_version.as_deref());
    single_summary(
        name,
        Some(version.as_str()),
        Some(op_badge(pkg.operation)),
        None,
    )
}

fn version_label(pkg: &InstallPackage) -> Element<'static> {
    muted(text::monotext(version_change(
        pkg.old_version.as_deref(),
        pkg.new_version.as_deref(),
    )))
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

fn op_badge(operation: PackageOp) -> (&'static str, fn(&cosmic::Theme) -> Color) {
    match operation {
        PackageOp::Install => ("Install", success_color),
        PackageOp::Upgrade => ("Upgrade", accent_color),
        PackageOp::Reinstall => ("Reinstall", accent_color),
        PackageOp::Downgrade => ("Downgrade", accent_color),
        PackageOp::Remove => ("Remove", destructive_color),
    }
}

fn op_pill(operation: PackageOp) -> Element<'static> {
    let (label, color) = op_badge(operation);
    pill(label, color)
}
