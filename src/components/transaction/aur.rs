use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Instant;

use cosmic::iced::alignment::Vertical;
use cosmic::widget::{Column, Row, space, text};
use pakajo::download::TransferState;
use pakajo::progress::{
    AurStage, BuildPackage, BuildStatus, InstallKind, ResolvedDep, ordered_aur_stages,
};
use pakajo::utils::format_elapsed;

use super::TransactionMessage;
use super::finalize::{finalize_log, finalize_section};
use super::install::{install_section, install_view};
use super::shared::{
    ResolvedEntry, counter_suffix, destructive_color, download_view, mono_text, muted, percent,
    resolve_empty_view, resolve_package_row, single_summary, summary_text, tinted,
};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view, toggle_button};
use crate::Element;
use crate::components::ansi::build_line_element;

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = match model.kind {
        InstallKind::Install if model.targets.len() > 1 => {
            format!("Installing {} packages", model.targets.len())
        }
        InstallKind::Install => format!("Installing {}", model.name),
        InstallKind::Remove => format!("Removing {}", model.name),
        InstallKind::Upgrade => format!("Upgrading {}", model.name),
    };
    let mut sections = Vec::new();
    for (i, stage) in ordered_aur_stages().iter().enumerate() {
        let state = model.aur_stage_state(*stage);
        if state != StageState::Failed {
            match *stage {
                AurStage::Deps if !model.deps_section_visible() => continue,
                AurStage::Build | AurStage::Install | AurStage::Finalize
                    if !model.artifact_sections_visible() =>
                {
                    continue;
                }
                _ => {}
            }
        }
        let mut section = match *stage {
            AurStage::Deps => deps_section(model, state),
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
    sections_view(title, sections, finished, model.is_sysupgrade())
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

pub(super) fn build_section(model: &TransactionModel, state: StageState) -> Section<'_> {
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
                section.content = Some(build_list_view(&ordered, &model.expanded_cards, model.now));
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
    let suffix = Row::new()
        .spacing(6)
        .push(counter_suffix(done, ordered.len(), "packages"))
        .push_maybe(elapsed.map(elapsed_label));
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
    let lines = if expanded { usize::MAX } else { 5 };
    Column::new()
        .spacing(4)
        .push(header)
        .push(tail_view(tail, lines))
        .into()
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

fn with_in_flight_download<'a>(
    mut section: Section<'a>,
    download: &'a TransferState,
) -> Section<'a> {
    if download.total == 0 || download.done >= download.total {
        return section;
    }
    let view = download_view(download);
    section.content = Some(match section.content {
        Some(existing) => Column::new().spacing(8).push(view).push(existing).into(),
        None => Column::new().spacing(8).push(view).into(),
    });
    section.suffix = Some(counter_suffix(download.done, download.total, "packages"));
    if download.bytes_total.max(0) > 0 {
        section.progress =
            Some(percent(download.bytes_done.max(0), download.bytes_total.max(0)) as f32);
    }
    section
}

fn deps_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = install_section(&model.aur.repo_deps.install, state, model.kind);
    section.label = "Dependencies";
    let download = &model.aur.repo_deps.download;
    if state == StageState::Active {
        section = with_in_flight_download(section, download);
    }
    if state == StageState::Done {
        let count = model.aur.repo_deps.install.order.len();
        section.summary = Some(summary_text(&[format!("{count} packages")]));
        let has_install = !model.aur.repo_deps.install.order.is_empty();
        let has_finalize = !model.aur.repo_deps.finalize.is_empty();
        if !has_install && !has_finalize {
            section.content = None;
        } else {
            let col = Column::new()
                .spacing(8)
                .push_maybe(has_install.then(|| install_view(&model.aur.repo_deps.install, true)))
                .push_maybe(has_finalize.then(|| finalize_log(&model.aur.repo_deps.finalize)));
            section.content = Some(col.into());
        }
    }
    if state == StageState::Failed && !model.aur.repo_deps.install.order.is_empty() {
        section.content = Some(install_view(&model.aur.repo_deps.install, false));
    }
    section
}

fn aur_install_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = install_section(&model.aur.install, state, model.kind);
    let download = &model.aur.download;
    if state == StageState::Active {
        section = with_in_flight_download(section, download);
    }
    if state == StageState::Failed && !model.aur.install.order.is_empty() {
        section.content = Some(install_view(&model.aur.install, false));
    }
    section
}

pub(super) fn failure_note(message: &str) -> Element<'static> {
    tinted(text::monotext(message.to_string()), destructive_color)
}
