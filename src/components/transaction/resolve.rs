use std::borrow::Cow;

use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, text};
use pakajo::events::{SummaryPackage, TransactionSummary, target_version};
use pakajo::progress::RepoState;
use pakajo::utils::format_bytes;

use crate::Element;

use super::accordion::Section;
use super::shared::{
    ResolvedEntry, accent_color, format_signed_bytes, muted, muted_color, pill, resolve_empty_view,
    resolve_package_row, resolve_single_suffix, success_color,
};
use super::state::StageState;

pub(super) fn resolve_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Resolve", state);
    match state {
        StageState::Active => section.header_suffix = Some(resolve_active_view(repo)),
        StageState::Done => match repo.manifest.as_ref() {
            Some(summary) if summary.packages.len() == 1 => {
                section.header_suffix = Some(single_package_suffix(&summary.packages[0]));
            }
            Some(summary) => {
                section.content = Some(resolve_done_view(summary));
                section.header_suffix = Some(multi_package_suffix(summary));
            }
            None => {}
        },
        _ => {}
    }
    section
}

fn resolve_active_view(state: &RepoState) -> Element<'_> {
    match state.resolve_step() {
        0 => status_row("Preparing...", None),
        1 => status_row("Resolving dependencies...", Some(1)),
        _ => status_row("Checking dependencies...", Some(2)),
    }
}

fn resolve_done_view(summary: &TransactionSummary) -> Element<'_> {
    if summary.packages.is_empty() {
        return resolve_empty_view();
    }
    let mut col = Column::new().spacing(10);
    let mut ordered: Vec<&SummaryPackage> = summary.packages.iter().collect();
    ordered.sort_by(|a, b| (!a.is_removal, &a.name).cmp(&(!b.is_removal, &b.name)));
    for pkg in ordered {
        col = col.push(resolve_package_row(&summary_entry(pkg)));
    }
    col.into()
}

fn status_row(label: &str, done_dots: Option<usize>) -> Element<'_> {
    let mut row = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(label)));
    if let Some(done) = done_dots {
        row = row.push(step_dots(done));
    }
    row.into()
}

fn step_dots(done: usize) -> Element<'static> {
    let mut row = Row::new().align_y(Vertical::Center).spacing(6);
    for i in 0..3 {
        row = row.push(step_dot(i, done));
    }
    row.into()
}

fn step_dot(index: usize, done: usize) -> Element<'static> {
    let color_fn: fn(&cosmic::Theme) -> Color = if index < done {
        success_color
    } else if index == done {
        accent_color
    } else {
        muted_color
    };
    container(text(""))
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(color_fn(theme))),
            border: Border {
                radius: theme.cosmic().corner_radii.radius_xs.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn single_package_suffix(pkg: &SummaryPackage) -> Element<'_> {
    let version = target_version(pkg);
    let entry = ResolvedEntry {
        qualified: Cow::Borrowed(pkg.name.as_str()),
        old_version: None,
        new_version: Some(version),
        net_size: Some(net_bytes(pkg)),
    };
    resolve_single_suffix(&entry)
}

fn summary_entry(pkg: &SummaryPackage) -> ResolvedEntry<'_> {
    let qualified: Cow<'_, str> = match pkg.repository.as_deref() {
        Some(repo) => Cow::Owned(format!("{repo}/{}", pkg.name)),
        None => Cow::Borrowed(pkg.name.as_str()),
    };
    ResolvedEntry {
        qualified,
        old_version: pkg.old_version.as_deref(),
        new_version: (!pkg.is_removal).then_some(pkg.new_version.as_str()),
        net_size: Some(net_bytes(pkg)),
    }
}

fn multi_package_suffix(summary: &TransactionSummary) -> Element<'_> {
    let count_label = format!("{} pkgs", summary.packages.len());
    let download_label = format!("↓ {}", format_bytes(summary.total_download_size));
    let net = summary.total_installed_size - summary.total_removed_size;
    let net_label = if net == 0 {
        "No size change".to_string()
    } else {
        format!("{} disk", format_signed_bytes(net))
    };
    Row::new()
        .align_y(Vertical::Center)
        .spacing(6)
        .push(pill(count_label, accent_color))
        .push(pill(download_label, accent_color))
        .push(pill(net_label, success_color))
        .into()
}

fn net_bytes(pkg: &SummaryPackage) -> i64 {
    if pkg.is_removal {
        -pkg.installed_size
    } else {
        pkg.installed_size - pkg.old_installed_size
    }
}
