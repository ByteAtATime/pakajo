use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, space, text};
use pakajo::events::PackageOp;
use pakajo::transaction_state::{InstallPackage, RepoState};

use crate::Element;
use crate::components::icons::circle_check;

use super::accordion::Section;
use super::shared::{accent_color, muted, on_color, tinted};
use super::state::StageState;

const GROUP_GAP: f32 = 8.0;

pub(super) fn install_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section {
        label: "Install",
        state,
        content: None,
        header_suffix: None,
    };
    match state {
        StageState::Done if repo.install_order.len() == 1 => {
            section.header_suffix = Some(install_single_suffix(repo));
        }
        StageState::Active if !repo.install_order.is_empty() => {
            section.content = Some(install_view(repo, false));
            if repo.install_order.len() > 1 {
                section.header_suffix = Some(install_counter_suffix(repo, false));
            }
        }
        StageState::Done if !repo.install_order.is_empty() => {
            section.content = Some(install_view(repo, true));
            section.header_suffix = Some(install_counter_suffix(repo, true));
        }
        _ => {}
    }
    section
}

fn install_view(state: &RepoState, done: bool) -> Element<'_> {
    let (finished, active): (Vec<_>, Vec<_>) = state
        .install_order
        .iter()
        .filter_map(|name| state.install_packages.get(name).map(|pkg| (name, pkg)))
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

fn install_counter_suffix(state: &RepoState, done: bool) -> Element<'_> {
    let total = state.install_order.len();
    let finished = if done {
        total
    } else {
        state
            .install_packages
            .values()
            .filter(|pkg| pkg.completed)
            .count()
    };
    muted(text(format!("{finished} / {total} packages")))
}

fn install_single_suffix(state: &RepoState) -> Element<'_> {
    let entry = state
        .install_order
        .first()
        .and_then(|name| state.install_packages.get(name).map(|pkg| (name, pkg)));
    let Some((name, pkg)) = entry else {
        return muted(text("1 package"));
    };
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(package_name(name))
        .push(muted(text(version_text(pkg)).font(cosmic::font::mono())))
        .push(op_pill(pkg.operation))
        .into()
}

fn package_row<'a>(name: &'a str, pkg: &'a InstallPackage) -> Element<'a> {
    let top = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(package_name(name))
        .push(muted(text(version_text(pkg)).font(cosmic::font::mono())))
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
        .push(muted(text(version_text(pkg)).font(cosmic::font::mono())))
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

fn version_text(pkg: &InstallPackage) -> String {
    match (pkg.old_version.as_deref(), pkg.new_version.as_deref()) {
        (Some(old), Some(new)) => format!("{old} → {new}"),
        (None, Some(new)) => new.to_string(),
        (Some(old), None) => old.to_string(),
        (None, None) => String::new(),
    }
}

fn op_pill(operation: PackageOp) -> Element<'static> {
    let (label, color_fn): (_, fn(&cosmic::Theme) -> Color) = match operation {
        PackageOp::Install => ("Install", success_color),
        PackageOp::Upgrade => ("Upgrade", accent_color),
        PackageOp::Reinstall => ("Reinstall", accent_color),
        PackageOp::Downgrade => ("Downgrade", accent_color),
        PackageOp::Remove => ("Remove", destructive_color),
    };
    container(text(label))
        .padding([2.0, 8.0])
        .style(move |theme: &cosmic::Theme| {
            let colored = color_fn(theme);
            container::Style {
                text_color: Some(colored),
                background: Some(Background::Color(Color { a: 0.10, ..colored })),
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn success_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().success.base)
}

fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}
