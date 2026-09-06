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
    ResolvedEntry, accent_color, counter_suffix, destructive_color, mono_text, muted, pill,
    resolve_empty_view, resolve_package_row, resolve_single_suffix, success_color, tinted,
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
            AurStage::Finalize => aur_finalize_section(model, state),
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
            section.header_suffix = Some(muted(text(format!("{} resolved", ordered.len()))));
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
                section.header_suffix = Some(resolve_single_suffix(&aur_entry(name, dep)));
                return section;
            }
            section.content = Some(resolve_list_view(&ordered));
            section.header_suffix = Some(resolve_suffix(&ordered));
        }
        _ => {}
    }
    section
}

fn resolve_suffix(ordered: &[(&String, &ResolvedDep)]) -> Element<'static> {
    let aur = ordered
        .iter()
        .filter(|(_, dep)| dep.repository.is_none())
        .count();
    Row::new()
        .spacing(6)
        .push(pill(format!("{} pkgs", ordered.len()), accent_color))
        .push(pill(format!("{aur} AUR"), success_color))
        .into()
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
            section.header_suffix = Some(build_progress_suffix(&ordered, done, elapsed));
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
                let mut suffix =
                    Row::new()
                        .spacing(6)
                        .push(resolve_single_suffix(&ResolvedEntry {
                            qualified: Cow::Borrowed(name.as_str()),
                            old_version: None,
                            new_version: None,
                            net_size: None,
                        }));
                if let Some(duration) = model
                    .aur
                    .build_ended
                    .map(|end| end.saturating_duration_since(entry.started))
                {
                    suffix = suffix.push(elapsed_label(duration));
                }
                section.header_suffix = Some(suffix.into());
                return section;
            }
            let elapsed = match (ordered.first(), model.aur.build_ended) {
                (Some((_, first)), Some(end)) => Some(end.saturating_duration_since(first.started)),
                _ => None,
            };
            section.content = Some(build_list_view(&ordered, &model.expanded_cards, model.now));
            section.header_suffix = Some(build_progress_suffix(&ordered, done, elapsed));
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
        .push(counter_suffix(done, ordered.len(), "built"));
    if let Some(duration) = elapsed {
        suffix = suffix.push(elapsed_label(duration));
    }
    suffix.into()
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

fn aur_finalize_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = finalize_section(&model.aur.finalize, state);
    if state == StageState::Done && model.aur.finalize.is_empty() {
        section.content = Some(resolve_empty_view());
    }
    section
}

fn failure_note(message: &str) -> Element<'static> {
    tinted(text::monotext(message.to_string()), destructive_color)
}
