use crate::search_view::centered;
use crate::session::UpdatesState;
use crate::updates::{PendingUpdates, RepoUpgrade};
use crate::utils::format_bytes;
use gpui::*;
use gpui_component::{ActiveTheme as _, StyledExt as _, spinner::Spinner};
use std::sync::Arc;

pub(crate) struct UpdatesView {
    scroll_handle: UniformListScrollHandle,
}

impl UpdatesView {
    pub(crate) fn new() -> Self {
        Self {
            scroll_handle: UniformListScrollHandle::default(),
        }
    }

    pub(crate) fn render(
        &self,
        updates: &PendingUpdates,
        state: UpdatesState,
        aur_error: Option<&str>,
        cx: &App,
    ) -> AnyElement {
        let muted_fg = cx.theme().muted_foreground;

        if matches!(state, UpdatesState::Loading) && updates.repo.is_empty() {
            return centered()
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(Spinner::new())
                        .child(div().text_color(muted_fg).child("Checking for updates…")),
                )
                .into_any_element();
        }

        if let UpdatesState::Error(message) = &state {
            let detail = message.clone();
            return centered()
                .v_flex()
                .gap_1()
                .text_color(cx.theme().danger_foreground)
                .child(div().child("Couldn't check for updates"))
                .children((!detail.is_empty()).then(|| div().text_color(muted_fg).child(detail)))
                .into_any_element();
        }

        let repo_count = updates.repo.len();

        if matches!(state, UpdatesState::Idle) && repo_count == 0 && updates.aur.is_empty() {
            return centered()
                .text_color(muted_fg)
                .child("Your system is up to date")
                .into_any_element();
        }

        let list: AnyElement = if repo_count == 0 {
            centered()
                .text_color(muted_fg)
                .child("No repository updates")
                .into_any_element()
        } else {
            let items = Arc::new(updates.repo.clone());
            uniform_list(
                "updates-repo-list",
                repo_count,
                move |range, _window, cx| {
                    let mut rows: Vec<AnyElement> = Vec::with_capacity(range.end - range.start);
                    for index in range {
                        rows.push(repo_upgrade_row(index, &items[index], cx).into_any_element());
                    }
                    rows
                },
            )
            .track_scroll(&self.scroll_handle)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .size_full()
            .v_flex()
            .min_h_0()
            .child(list)
            .children(aur_error.map(|message| {
                div()
                    .px_3()
                    .py_2()
                    .text_color(muted_fg)
                    .child(format!("AUR check failed: {message}"))
            }))
            .into_any_element()
    }
}

fn repo_upgrade_row(index: usize, upgrade: &RepoUpgrade, cx: &App) -> Stateful<Div> {
    let muted_fg = cx.theme().muted_foreground;
    let muted_bg = cx.theme().muted;
    div()
        .id(format!("upd-{index}"))
        .v_flex()
        .gap_1()
        .px_3()
        .py_2()
        .hover(|s| s.bg(muted_bg.opacity(0.5)))
        .child(
            div()
                .h_flex()
                .items_baseline()
                .gap_2()
                .child(div().font_semibold().child(upgrade.name.clone()))
                .child(div().text_color(muted_fg).child(upgrade.repo.clone()))
                .child(
                    div()
                        .ml_auto()
                        .h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(div().child(format!("{} -> {}", upgrade.old, upgrade.new)))
                        .child(
                            div()
                                .text_color(muted_fg)
                                .child(format_bytes(upgrade.download_size)),
                        ),
                ),
        )
}
