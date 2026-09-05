use pakajo::transaction_state::RepoState;

use super::accordion::Section;
use super::shared::{counter_suffix, download_view};
use super::state::StageState;

pub(super) fn download_section(repo: &RepoState, state: StageState) -> Section<'_> {
    if repo.download.total == 0 {
        return Section {
            label: "Download",
            state,
            content: None,
            header_suffix: None,
        };
    }
    match state {
        StageState::Active | StageState::Done => Section {
            label: "Download",
            state,
            content: Some(download_view(&repo.download)),
            header_suffix: Some(counter_suffix(
                repo.download.done,
                repo.download.total,
                "packages",
            )),
        },
        _ => Section {
            label: "Download",
            state,
            content: None,
            header_suffix: None,
        },
    }
}
