use super::aur::build_section;
use super::failure::failure_card;
use super::finalize::finalize_section;
use super::install::install_section;
use super::resolve::prepare_section;
use super::shared::{cancelled_card, counter_suffix, download_view, percent};
use super::state::{StageState, TransactionModel, TransactionStatus};
use super::stepper::{Section, sections_view};
use crate::Element;
use pakajo::dispatch::exec::ChildOutcome;
use pakajo::progress::{AurStage, InstallKind, RepoStage, RepoState};

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
    for (i, stage) in model.stages.iter().enumerate() {
        let state = model.stage_state(i);
        let section = match *stage {
            RepoStage::Validate => continue,
            RepoStage::Resolve => {
                prepare_section(&model.repo_state, prepare_state(model)).with_toggle_index(1)
            }
            RepoStage::Download => download_section(&model.repo_state, state).with_toggle_index(i),
            RepoStage::Install => {
                install_section(&model.repo_state.install, state, model.kind).with_toggle_index(i)
            }
            RepoStage::Finalize => {
                finalize_section(&model.repo_state.finalize, state).with_toggle_index(i)
            }
        };
        let expanded = if *stage == RepoStage::Resolve {
            model.expanded.contains(&1)
        } else {
            model.expanded.contains(&i)
        };
        sections.push((section, expanded));
    }
    if model.is_sysupgrade() && !model.aur.build_order.is_empty() {
        let build_index = model.stages.len();
        let section = build_section(model, model.aur_stage_state(AurStage::Build))
            .with_toggle_index(build_index);
        sections.push((section, model.expanded.contains(&build_index)));
    }
    let finished = matches!(model.status, TransactionStatus::Done(_));
    let failure = model
        .failure()
        .map(|failure| failure_card(&model.name, model.kind, failure));
    let cancelled = matches!(
        model.status,
        TransactionStatus::Done(ChildOutcome::Cancelled)
    )
    .then(|| cancelled_card(&model.name, cancelled_summary(model)));
    let cancel_eligible = model.cancel_eligible();
    sections_view(
        title,
        failure,
        sections,
        finished,
        model.is_sysupgrade(),
        cancel_eligible,
        cancelled,
    )
}

fn prepare_state(model: &TransactionModel) -> StageState {
    let resolve_state = model.stage_state(0);
    let validate_state = model.stage_state(1);
    if validate_state == StageState::Done {
        return StageState::Done;
    }
    if resolve_state == StageState::Failed || validate_state == StageState::Failed {
        return StageState::Failed;
    }
    if resolve_state == StageState::Done {
        return StageState::Active;
    }
    resolve_state
}

pub(super) fn download_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Download", state);
    if repo.download.total == 0 {
        return section;
    }
    match state {
        StageState::Active => {
            section.content = Some(download_view(&repo.download));
            section.suffix = Some(counter_suffix(
                repo.download.done,
                repo.download.total,
                "packages",
            ));
            if repo.download.bytes_total.max(0) > 0 {
                section.progress = Some(percent(
                    repo.download.bytes_done.max(0),
                    repo.download.bytes_total.max(0),
                ) as f32);
            }
        }
        StageState::Done => {
            section.content = Some(download_view(&repo.download));
            section.summary = Some(counter_suffix(
                repo.download.done,
                repo.download.total,
                "packages",
            ));
        }
        _ => {}
    }
    section
}

fn cancelled_summary(model: &TransactionModel) -> String {
    let progress = match model.stages.get(model.current_idx) {
        Some(RepoStage::Download) => {
            let download = &model.repo_state.download;
            Some(format!("{}/{}", download.done, download.total))
        }
        Some(RepoStage::Install) => {
            let install = &model.repo_state.install;
            let done = install
                .packages
                .values()
                .filter(|package| package.completed)
                .count();
            Some(format!("{done}/{}", install.order.len()))
        }
        _ => None,
    };
    super::shared::cancelled_summary(Some(cancelled_stage_label(model)), progress)
}

fn cancelled_stage_label(model: &TransactionModel) -> &'static str {
    match model.stages.get(model.current_idx) {
        Some(RepoStage::Resolve) | Some(RepoStage::Validate) => "prepare",
        Some(RepoStage::Download) => "download",
        Some(RepoStage::Install) => match model.kind {
            InstallKind::Remove => "remove",
            InstallKind::Install | InstallKind::Upgrade => "install",
        },
        Some(RepoStage::Finalize) => "finalize",
        None => "finalize",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pakajo::events::InstallEvent;
    use pakajo::package::PackageSource;

    fn downloading_model() -> TransactionModel {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.status = TransactionStatus::Running;
        model.apply_event(&InstallEvent::TransactionSummary(
            pakajo::events::TransactionSummary {
                packages: Vec::new(),
                total_download_size: 0,
                total_installed_size: 0,
                total_removed_size: 0,
            },
        ));
        model.apply_event(&InstallEvent::RetrievingPackages {
            num: 5,
            total_bytes: 500,
        });
        for name in ["a.pkg", "b.pkg"] {
            model.apply_event(&InstallEvent::DownloadInit {
                filename: name.to_string(),
                optional: false,
            });
            model.apply_event(&InstallEvent::DownloadCompleted {
                filename: name.to_string(),
                total: 100,
                result: pakajo::events::DownloadResult::Success,
            });
        }
        model
    }

    #[test]
    fn cancelled_download_summary_counts_files() {
        let mut model = downloading_model();
        assert_eq!(model.repo_state.download.done, 2);
        assert_eq!(model.repo_state.download.total, 5);
        model.finish(ChildOutcome::Cancelled);
        assert_eq!(cancelled_summary(&model), "Cancelled during download, 2/5");
    }

    #[test]
    fn cancelled_prepare_summary_has_no_counts() {
        let mut model = TransactionModel::new(
            "firefox".to_string(),
            PackageSource::Repo,
            InstallKind::Install,
        );
        model.status = TransactionStatus::Running;
        model.finish(ChildOutcome::Cancelled);
        assert_eq!(cancelled_summary(&model), "Cancelled during prepare");
    }
}
