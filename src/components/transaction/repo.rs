use cosmic::widget::{Column, scrollable, text};
use pakajo::transaction_state::{InstallKind, RepoStage};

use super::accordion::{action_footer, stage_row};
use super::download::download_section;
use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::resolve_section;
use super::state::{TransactionModel, TransactionStatus};
use super::validate::validate_section;
use crate::Element;

pub(super) fn view(model: &TransactionModel) -> Element<'_> {
    let title = match model.kind {
        InstallKind::Install => format!("Installing {}", model.name),
        InstallKind::Remove => format!("Removing {}", model.name),
        InstallKind::Upgrade => format!("Upgrading {}", model.name),
    };
    let mut panels = Column::new().spacing(6);
    for (i, stage) in model.stages.iter().enumerate() {
        let state = model.stage_state(i);
        let expanded = model.expanded.contains(&i);
        let section = match *stage {
            RepoStage::Resolve => resolve_section(&model.repo_state, state),
            RepoStage::Validate => validate_section(&model.repo_state, state),
            RepoStage::Download => download_section(&model.repo_state, state),
            RepoStage::Install => install_section(&model.repo_state, state),
            RepoStage::Finalize => finalize_section(&model.repo_state, state),
        };
        panels = panels.push(stage_row(section, expanded, i));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if matches!(model.status, TransactionStatus::Done(_)) {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}
