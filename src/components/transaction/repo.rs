use cosmic::widget::{Column, scrollable, text};
use pakajo::transaction_state::{InstallKind, RepoStage, RepoState};

use super::accordion::{Section, action_footer, stage_row};
use super::shared::download_view;
use super::state::{StageState, TransactionModel, TransactionStatus};
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
        let content = (state == StageState::Active).then(|| active_view(&model.repo_state, *stage));
        let section = Section {
            label: stage_label(*stage),
            state,
            content,
        };
        panels = panels.push(stage_row(section, model.expanded.contains(&i), i));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if matches!(model.status, TransactionStatus::Done(_)) {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

fn active_view(state: &RepoState, stage: RepoStage) -> Element<'_> {
    match stage {
        RepoStage::Download => download_view(state),
        _ => text(format!("running phase {}", stage_label(stage))).into(),
    }
}

fn stage_label(stage: RepoStage) -> &'static str {
    match stage {
        RepoStage::Resolve => "Resolve",
        RepoStage::Validate => "Validate",
        RepoStage::Download => "Download",
        RepoStage::Install => "Install",
        RepoStage::Finalize => "Finalize",
    }
}
