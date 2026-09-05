use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, space, text};
use pakajo::events::{SummaryPackage, TransactionSummary, target_version};
use pakajo::transaction_state::RepoState;
use pakajo::utils::format_bytes;

use crate::Element;

use super::accordion::Section;
use super::shared::{accent_color, muted, muted_color, on_color, success_color, tinted};
use super::state::StageState;

pub(super) fn resolve_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section {
        label: "Resolve",
        state,
        content: None,
        header_suffix: None,
    };
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
        return muted(text("Nothing to do"));
    }
    let mut col = Column::new().spacing(10);
    let mut ordered: Vec<&SummaryPackage> = summary.packages.iter().collect();
    ordered.sort_by_key(|p| (!p.is_removal, p.name.clone()));
    for pkg in ordered {
        col = col.push(package_row(pkg));
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
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn single_package_suffix(pkg: &SummaryPackage) -> Element<'_> {
    let version = target_version(pkg);
    let name_version = if version.is_empty() {
        pkg.name.clone()
    } else {
        format!("{} {version}", pkg.name)
    };
    let net = net_bytes(pkg);
    let size_color: fn(&cosmic::Theme) -> Color = if net >= 0 {
        accent_color
    } else {
        success_color
    };
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(name_version).font(cosmic::font::mono())))
        .push(metric_pill(format_signed_bytes(net), size_color))
        .into()
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
        .push(metric_pill(count_label, accent_color))
        .push(metric_pill(download_label, accent_color))
        .push(metric_pill(net_label, success_color))
        .into()
}

fn package_row(pkg: &SummaryPackage) -> Element<'_> {
    let qualified = match pkg.repository.as_deref() {
        Some(repo) => format!("{repo}/{}", pkg.name),
        None => pkg.name.clone(),
    };
    let version_text = if pkg.is_removal {
        pkg.old_version.clone().unwrap_or_default()
    } else if let Some(old) = pkg.old_version.as_deref() {
        format!("{old} → {}", pkg.new_version)
    } else {
        pkg.new_version.clone()
    };
    let size = format_signed_bytes(net_bytes(pkg));
    let left = tinted(text(qualified).font(cosmic::font::mono()), on_color);
    let right = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(version_text).font(cosmic::font::mono())))
        .push(text(size));
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(left)
        .push(space::horizontal())
        .push(right)
        .into()
}

fn net_bytes(pkg: &SummaryPackage) -> i64 {
    if pkg.is_removal {
        -pkg.installed_size
    } else {
        pkg.installed_size - pkg.old_installed_size
    }
}

fn format_signed_bytes(value: i64) -> String {
    if value < 0 {
        format!("-{}", format_bytes(value.abs()))
    } else {
        format!("+{}", format_bytes(value))
    }
}

fn metric_pill(label: String, color_fn: fn(&cosmic::Theme) -> Color) -> Element<'static> {
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
