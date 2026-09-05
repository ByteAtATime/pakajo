use pakajo::transaction_state::{InstallKind, RepoStage};

use super::accordion::sections_view;
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
    let sections = model
        .stages
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let state = model.stage_state(i);
            let section = match *stage {
                RepoStage::Resolve => resolve_section(&model.repo_state, state),
                RepoStage::Validate => validate_section(&model.repo_state, state),
                RepoStage::Download => download_section(&model.repo_state, state),
                RepoStage::Install => install_section(&model.repo_state.install, state),
                RepoStage::Finalize => finalize_section(&model.repo_state.finalize, state),
            };
            (section, model.expanded.contains(&i))
        })
        .collect();
    let finished = matches!(model.status, TransactionStatus::Done(_));
    sections_view(title, sections, finished)
}
