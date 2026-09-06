use std::borrow::Cow;

use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::progress_bar;
use cosmic::widget::{Column, Row, text};
use pakajo::events::{SummaryPackage, TransactionSummary, target_version};
use pakajo::progress::{RepoState, VALIDATE_TOTAL};
use pakajo::utils::format_bytes;

use crate::Element;

use super::shared::{
    ResolvedEntry, accent_color, format_signed_bytes, muted, pill, resolve_empty_view,
    resolve_package_row, resolve_single_suffix, success_color,
};
use super::state::StageState;
use super::stepper::Section;

pub(super) fn prepare_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Prepare", state);
    match state {
        StageState::Active => {
            section.header_suffix = Some(prepare_active_view(repo));
            if let Some(summary) = repo.manifest.as_ref() {
                section.content = Some(resolve_done_view(summary));
            }
        }
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

fn prepare_active_view(repo: &RepoState) -> Element<'_> {
    let validate_count = repo.validate.count();
    let units = (repo.resolve.started as usize) + (repo.resolve.checking as usize) + validate_count;
    let total = 2 + VALIDATE_TOTAL;
    let pct = units as f32 / total as f32 * 100.0;
    let label = if validate_count > 0 {
        format!(
            "{} ({}/{})",
            repo.validate.label, validate_count, VALIDATE_TOTAL
        )
    } else if repo.resolve.checking {
        "Checking dependencies...".to_string()
    } else if repo.resolve.started {
        "Resolving dependencies...".to_string()
    } else {
        "Preparing...".to_string()
    };
    let bar = progress_bar(0.0..=100.0, pct)
        .length(Length::Fixed(120.0))
        .girth(6.0);
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(label)))
        .push(bar)
        .into()
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
