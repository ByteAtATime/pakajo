use std::borrow::Cow;

use cosmic::iced::Color;
use cosmic::widget::{Column, text};
use pakajo::events::{SummaryPackage, TransactionSummary, target_version};
use pakajo::progress::{RepoState, VALIDATE_TOTAL};
use pakajo::utils::format_bytes;

use crate::Element;

use super::shared::{
    ResolvedEntry, accent_color, format_signed_bytes, muted, resolve_empty_view,
    resolve_package_row, single_summary, success_color, summary_text,
};
use super::state::StageState;
use super::stepper::Section;

pub(super) fn prepare_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Prepare", state);
    match state {
        StageState::Active => {
            section.suffix = Some(muted(text(prepare_label(repo))));
            section.progress = Some(prepare_percent(repo));
            if let Some(summary) = repo.manifest.as_ref() {
                section.content = Some(resolve_done_view(summary));
            }
        }
        StageState::Done => match repo.manifest.as_ref() {
            Some(summary) if summary.packages.len() == 1 => {
                let pkg = &summary.packages[0];
                let net = net_bytes(pkg);
                let size_color: fn(&cosmic::Theme) -> Color = if net >= 0 {
                    accent_color
                } else {
                    success_color
                };
                let size_label = format_signed_bytes(net);
                section.summary = Some(single_summary(
                    &pkg.name,
                    Some(target_version(pkg)),
                    Some((size_label, size_color)),
                    None,
                ));
            }
            Some(summary) => {
                section.content = Some(resolve_done_view(summary));
                section.summary = Some(multi_package_suffix(summary));
            }
            None => {}
        },
        _ => {}
    }
    section
}

fn prepare_label(repo: &RepoState) -> String {
    let validate_count = repo.validate.count();
    if validate_count > 0 {
        repo.validate.label.to_string()
    } else if repo.resolve.checking {
        "Checking dependencies...".to_string()
    } else if repo.resolve.started {
        "Resolving dependencies...".to_string()
    } else {
        "Preparing...".to_string()
    }
}

fn prepare_percent(repo: &RepoState) -> f32 {
    let validate_count = repo.validate.count();
    let units = (repo.resolve.started as usize) + (repo.resolve.checking as usize) + validate_count;
    let total = 2 + VALIDATE_TOTAL;
    units as f32 / total as f32 * 100.0
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
    let count = summary.packages.len();
    let net = summary.total_installed_size - summary.total_removed_size;
    let net_label = if net == 0 {
        "No size change".to_string()
    } else {
        format!("{} disk", format_signed_bytes(net))
    };
    let parts = vec![
        format!("{count} / {count} packages"),
        format!("↓ {}", format_bytes(summary.total_download_size)),
        net_label,
    ];
    summary_text(&parts)
}

fn net_bytes(pkg: &SummaryPackage) -> i64 {
    if pkg.is_removal {
        -pkg.installed_size
    } else {
        pkg.installed_size - pkg.old_installed_size
    }
}
