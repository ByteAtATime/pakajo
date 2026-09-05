use std::borrow::Cow;

use cosmic::widget::{Column, Row, scrollable, text};
use pakajo::transaction_state::{AurStage, InstallKind, ResolvedDep, ordered_aur_stages};

use super::SYSTEM_AUR_NAME;
use super::accordion::{Section, action_footer, stage_row};
use super::shared::{
    ResolvedEntry, accent_color, muted, pill, resolve_empty_view, resolve_package_row,
    resolve_single_suffix, success_color,
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
            AurStage::Build => pending_section("Build", state),
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

fn pending_section(label: &'static str, state: StageState) -> Section<'static> {
    Section {
        label,
        state,
        content: None,
        header_suffix: None,
    }
}
