use std::borrow::Cow;
use std::collections::HashSet;

use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::widget::{Column, Row, button, scrollable, space, text};
use pakajo::transaction_state::{
    AurStage, BuildPackage, BuildStatus, InstallKind, ResolvedDep, ordered_aur_stages,
};

use super::SYSTEM_AUR_NAME;
use super::TransactionMessage;
use super::accordion::{Section, action_footer, stage_row};
use super::shared::{
    ResolvedEntry, accent_color, counter_suffix, muted, on_color, pill, resolve_empty_view,
    resolve_package_row, resolve_single_suffix, success_color, tinted,
};
use super::state::{StageState, TransactionModel, TransactionStatus};
use crate::Element;

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
    let mut panels = Column::new().spacing(6);
    for (i, stage) in ordered_aur_stages().iter().enumerate() {
        let state = model.aur_stage_state(*stage);
        let section = match *stage {
            AurStage::Resolve => resolve_section(model, state),
            AurStage::Build => build_section(model, state),
            AurStage::Install => pending_section("Install", state),
            AurStage::Finalize => pending_section("Finalize", state),
        };
        panels = panels.push(stage_row(section, model.expanded.contains(&i), i));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if matches!(model.status, TransactionStatus::Done(_)) {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

fn resolve_section(model: &TransactionModel, state: StageState) -> Section<'_> {
    let mut section = Section {
        label: "Resolve",
        state,
        content: None,
        header_suffix: None,
    };
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
    let mut section = Section {
        label: "Build",
        state,
        content: None,
        header_suffix: None,
    };
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
            section.header_suffix = Some(counter_suffix(done, ordered.len(), "built"));
            if !ordered.is_empty() {
                section.content = Some(build_list_view(&ordered, &model.expanded_cards));
            }
        }
        StageState::Done => {
            if ordered.is_empty() {
                section.content = Some(resolve_empty_view());
                return section;
            }
            if ordered.len() == 1 {
                let (name, _) = ordered[0];
                section.header_suffix = Some(resolve_single_suffix(&ResolvedEntry {
                    qualified: Cow::Borrowed(name.as_str()),
                    old_version: None,
                    new_version: None,
                    net_size: None,
                }));
                return section;
            }
            section.content = Some(build_list_view(&ordered, &model.expanded_cards));
            section.header_suffix = Some(counter_suffix(done, ordered.len(), "built"));
        }
        _ => {}
    }
    section
}

fn tail_view(tail: &[String], lines: usize) -> Element<'static> {
    let mut col = Column::new().spacing(2);
    for line in &tail[tail.len().saturating_sub(lines)..] {
        col = col.push(muted(
            text(line.clone()).font(cosmic::font::mono()).size(12.0),
        ));
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

fn build_card(
    name: &str,
    status: BuildStatus,
    tail: &[String],
    expanded: bool,
) -> Element<'static> {
    let left = tinted(text(name.to_string()).font(cosmic::font::mono()), on_color);
    let row: Element<'static> = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(left)
        .push(space::horizontal())
        .push(muted(text(build_status_word(status))))
        .into();
    if tail.is_empty() {
        return row;
    }
    let header: Element<'static> = button::custom(row)
        .padding([2.0, 0.0])
        .width(Length::Fill)
        .class(cosmic::theme::Button::Transparent)
        .on_press(crate::Message::Transaction(
            TransactionMessage::ToggleBuildCard(name.to_string()),
        ))
        .into();
    if expanded || matches!(status, BuildStatus::Fetching | BuildStatus::Building) {
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
) -> Element<'static> {
    let mut col = Column::new().spacing(10);
    for (name, entry) in ordered {
        col = col.push(build_card(
            name,
            entry.status,
            &entry.tail,
            expanded.contains(*name),
        ));
    }
    col.into()
}

fn pending_section(label: &'static str, state: StageState) -> Section<'static> {
    Section {
        label,
        state,
        content: None,
        header_suffix: None,
    }
}
