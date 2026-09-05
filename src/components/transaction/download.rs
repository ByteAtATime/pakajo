use pakajo::progress::RepoState;

use super::accordion::Section;
use super::shared::{counter_suffix, download_view};
use super::state::StageState;

pub(super) fn download_section(repo: &RepoState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Download", state);
    if repo.download.total == 0 {
        return section;
    }
    if matches!(state, StageState::Active | StageState::Done) {
        section.content = Some(download_view(&repo.download));
        section.header_suffix = Some(counter_suffix(
            repo.download.done,
            repo.download.total,
            "packages",
        ));
    }
    section
}
