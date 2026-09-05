use cosmic::widget::{Column, scrollable, text};
use pakajo::transaction_state::{AurStage, InstallKind, ordered_aur_stages};

use super::SYSTEM_AUR_NAME;
use super::accordion::{Section, action_footer, stage_row};
use super::state::{TransactionModel, TransactionStatus};
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
        let section = Section {
            label: stage_label(*stage),
            state: model.aur_stage_state(*stage),
            content: None,
            header_suffix: None,
        };
        panels = panels.push(stage_row(section, model.expanded.contains(&i), i));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if matches!(model.status, TransactionStatus::Done(_)) {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

fn stage_label(stage: AurStage) -> &'static str {
    match stage {
        AurStage::Resolve => "Resolve",
        AurStage::Build => "Build",
        AurStage::Install => "Install",
        AurStage::Finalize => "Finalize",
    }
}
