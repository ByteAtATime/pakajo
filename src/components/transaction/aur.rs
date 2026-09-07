use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Instant;

use cosmic::iced::alignment::Vertical;
use cosmic::widget::{Column, Row, space, text};
use pakajo::progress::{
    AurStage, BuildPackage, BuildStatus, InstallKind, ResolvedDep, ordered_aur_stages,
};
use pakajo::utils::format_elapsed;

use super::SYSTEM_AUR_NAME;
use super::TransactionMessage;
use super::finalize::finalize_section;
use super::install::{install_section, install_view};
use super::shared::{
    ResolvedEntry, counter_suffix, destructive_color, mono_text, muted, resolve_empty_view,
    resolve_package_row, single_summary, summary_text, tinted,
};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view, toggle_button};
use crate::Element;
use crate::components::ansi::build_line_element;

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = if model.name == SYSTEM_AUR_NAME {
        "Upgrading AUR packages".to_string()
    } else {
        match model.kind {
            InstallKind::Install => format!("Installing {}", model.name),
            InstallKind::Remove => format!("Removing {}", model.name),
            InstallKind::Upgrade => format!("Upgrading {}", model.name),
        }
    };
    let mut sections = Vec::new();
    for (i, stage) in ordered_aur_stages().iter().enumerate() {
        let state = model.aur_stage_state(*stage);
        let mut section = match *stage {
            AurStage::Resolve => resolve_section(model, state),
            AurStage::Build => build_section(model, state),
            AurStage::Install => aur_install_section(model, state),
            AurStage::Finalize => finalize_section(&model.aur.finalize, state),
        };
        if state == StageState::Failed
            && let Some(message) = model.failure_message.as_deref()
        {
            let note = failure_note(message);
            section.content = Some(match section.content {
                Some(existing) => Column::new().spacing(10).push(note).push(existing).into(),
                None => note,
            });
        }
        let section = section.with_toggle_index(i);
        sections.push((section, model.expanded.contains(&i)));
    }
    let finished = matches!(model.status, TransactionStatus::Done(_));
    sections_view(title, sections, finished)
}

fn resolve_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = Section::new("Resolve", state);
    let ordered: Vec<(&String, &ResolvedDep)> = model
        .aur
        .dep_order
        .iter()
        .filter_map(|name| model.aur.deps.get(name).map(|dep| (name, dep)))
        .collect();
    match state {
        StageState::Active => {
            section.suffix = Some(muted(text(format!("{} resolved", ordered.len()))));
            if !ordered.is_empty() {
                section.content = Some(resolve_list_view(&ordered));
            }
        }
        StageState::Done => {
            if ordered.is_empty() {
                section.content = Some(resolve_empty_view());
                return section;
            }
            if ordered.len() == 1 {
                let (name, dep) = ordered[0];
                let qualified = match dep.repository.as_deref() {
                    Some(repo) => format!("{repo}/{name}"),
                    None => format!("aur/{name}"),
                };
                section.summary = Some(single_summary(
                    &qualified,
                    dep.version.as_deref(),
                    None::<(String, fn(&cosmic::Theme) -> cosmic::iced::Color)>,
                    None,
                ));
                return section;
            }
            section.content = Some(resolve_list_view(&ordered));
            section.summary = Some(resolve_summary(&ordered));
        }
        _ => {}
    }
    section
}

fn resolve_summary(ordered: &[(&String, &ResolvedDep)]) -> Element<'static> {
    let aur = ordered
        .iter()
        .filter(|(_, dep)| dep.repository.is_none())
        .count();
    let count = ordered.len();
    let mut parts = vec![format!("{count} / {count} packages")];
    if aur > 0 {
        parts.push(format!("{aur} AUR"));
    }
    summary_text(&parts)
}

fn aur_entry<'a>(name: &'a str, dep: &'a ResolvedDep) -> ResolvedEntry<'a> {
    let qualified = Cow::Owned(match dep.repository.as_deref() {
        Some(repo) => format!("{repo}/{name}"),
        None => format!("aur/{name}"),
    });
    ResolvedEntry {
        qualified,
        old_version: None,
        new_version: dep.version.as_deref(),
        net_size: None,
    }
}

fn resolve_list_view(ordered: &[(&String, &ResolvedDep)]) -> Element<'static> {
    let mut col = Column::new().spacing(10);
    for (name, dep) in ordered {
        col = col.push(resolve_package_row(&aur_entry(name, dep)));
    }
    col.into()
}

fn build_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = Section::new("Build", state);
    let ordered: Vec<(&String, &BuildPackage)> = model
        .aur
        .build_order
        .iter()
        .filter_map(|name| model.aur.builds.get(name).map(|entry| (name, entry)))
        .collect();
    let done = ordered
        .iter()
        .filter(|(_, entry)| entry.status == BuildStatus::Done)
        .count();
    match state {
        StageState::Active => {
            let elapsed = ordered
                .first()
                .map(|(_, first)| model.now.saturating_duration_since(first.started));
            section.suffix = Some(build_progress_suffix(&ordered, done, elapsed));
            if !ordered.is_empty() {
                section.content = Some(build_list_view(&ordered, &model.expanded_cards, model.now));
            }
        }
        StageState::Done => {
            if ordered.is_empty() {
                section.content = Some(resolve_empty_view());
                return section;
            }
            if ordered.len() == 1 {
                let (name, entry) = ordered[0];
                let elapsed = model
                    .aur
                    .build_ended
                    .map(|end| end.saturating_duration_since(entry.started))
                    .map(elapsed_label);
                section.summary = Some(single_summary(
                    name,
                    None,
                    None::<(String, fn(&cosmic::Theme) -> cosmic::iced::Color)>,
                    elapsed,
                ));
                return section;
            }
            let elapsed = match (ordered.first(), model.aur.build_ended) {
                (Some((_, first)), Some(end)) => Some(end.saturating_duration_since(first.started)),
                _ => None,
            };
            section.content = Some(build_list_view(&ordered, &model.expanded_cards, model.now));
            section.summary = Some(build_summary(&ordered, elapsed));
        }
        StageState::Failed if !ordered.is_empty() => {
            section.content = Some(build_list_view(&ordered, &model.expanded_cards, model.now));
        }
        _ => {}
    }
    section
}

fn build_progress_suffix(
    ordered: &[(&String, &BuildPackage)],
    done: usize,
    elapsed: Option<std::time::Duration>,
) -> Element<'static> {
    let mut suffix = Row::new()
        .spacing(6)
        .push(counter_suffix(done, ordered.len(), "packages"));
    if let Some(duration) = elapsed {
        suffix = suffix.push(elapsed_label(duration));
    }
    suffix.into()
}

fn build_summary(
    ordered: &[(&String, &BuildPackage)],
    elapsed: Option<std::time::Duration>,
) -> Element<'static> {
    let count = ordered.len();
    let mut parts = vec![format!("{count} / {count} packages")];
    if let Some(duration) = elapsed {
        parts.push(format_elapsed(duration));
    }
    summary_text(&parts)
}

fn elapsed_label(duration: std::time::Duration) -> Element<'static> {
    muted(text::monotext(format_elapsed(duration)))
}

fn tail_view(tail: &[String], lines: usize) -> Element<'static> {
    let mut col = Column::new().spacing(2);
    for line in &tail[tail.len().saturating_sub(lines)..] {
        col = col.push(build_line_element(line));
    }
    col.into()
}

fn build_status_word(status: BuildStatus) -> &'static str {
    match status {
        BuildStatus::Fetching => "fetching",
        BuildStatus::Building => "building",
        BuildStatus::Done => "done",
        BuildStatus::Failed => "failed",
    }
}

fn build_card(name: &str, entry: &BuildPackage, expanded: bool, now: Instant) -> Element<'static> {
    let mut header_row = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(mono_text(name))
        .push(space::horizontal())
        .push(muted(text(build_status_word(entry.status))));
    if let Some(duration) = match entry.status {
        BuildStatus::Fetching | BuildStatus::Building => {
            Some(now.saturating_duration_since(entry.started))
        }
        BuildStatus::Done | BuildStatus::Failed => entry.elapsed,
    } {
        header_row = header_row.push(elapsed_label(duration));
    }
    let row: Element<'static> = header_row.into();
    let tail = &entry.tail;
    if tail.is_empty() {
        return row;
    }
    let header: Element<'static> =
        toggle_button(row, TransactionMessage::ToggleBuildCard(name.to_string()));
    if expanded || matches!(entry.status, BuildStatus::Fetching | BuildStatus::Building) {
        let lines = if expanded { usize::MAX } else { 5 };
        return Column::new()
            .spacing(4)
            .push(header)
            .push(tail_view(tail, lines))
            .into();
    }
    header
}

fn build_list_view(
    ordered: &[(&String, &BuildPackage)],
    expanded: &HashSet<String>,
    now: Instant,
) -> Element<'static> {
    let mut col = Column::new().spacing(10);
    for (name, entry) in ordered {
        col = col.push(build_card(name, entry, expanded.contains(*name), now));
    }
    col.into()
}

fn aur_install_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = install_section(&model.aur.install, state);
    let download = &model.aur.download;
    if state == StageState::Active && download.total > 0 && download.done < download.total {
        let line = muted(text(format!(
            "Downloading {} / {} packages",
            download.done, download.total
        )));
        section.content = Some(match section.content {
            Some(existing) => Column::new().spacing(8).push(line).push(existing).into(),
            None => Column::new().spacing(8).push(line).into(),
        });
    }
    if state == StageState::Failed && !model.aur.install.order.is_empty() {
        section.content = Some(install_view(&model.aur.install, false));
    }
    section
}

fn failure_note(message: &str) -> Element<'static> {
    tinted(text::monotext(message.to_string()), destructive_color)
}
