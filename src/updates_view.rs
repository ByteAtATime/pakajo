use crate::icon::PakajoIcon;
use crate::search_view::centered;
use crate::session::UpdatesState;
use crate::updates::{PendingUpdates, RepoUpgrade};
use crate::upgrade::AurUpgradeCandidate;
use crate::utils::{format_bytes, version_diff};
use gpui::*;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Icon, StyledExt as _, spinner::Spinner};

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

        let items = build_updates_items(updates);

        div()
            .flex_1()
            .size_full()
            .v_flex()
            .min_h_0()
            .children(aur_error.map(|message| {
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .text_color(muted_fg)
                    .child(format!("AUR check failed: {message}"))
            }))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list("updates-list", items.len(), move |range, _window, cx| {
                            range.map(|i| render_updates_item(&items[i], cx)).collect()
                        })
                        .size_full()
                        .track_scroll(&self.scroll_handle),
                    )
                    .vertical_scrollbar(&self.scroll_handle),
            )
            .into_any_element()
    }
}

enum UpdatesEntry {
    Header(String),
    Repo(RepoUpgrade),
    Aur(AurUpgradeCandidate),
}

fn build_updates_items(updates: &PendingUpdates) -> Vec<UpdatesEntry> {
    let show_aur_section = !updates.aur.is_empty();
    let mut items =
        Vec::with_capacity(1 + updates.repo.len() + show_aur_section as usize + updates.aur.len());
    items.push(UpdatesEntry::Header(format!(
        "Repository ({})",
        updates.repo.len()
    )));
    items.extend(updates.repo.iter().cloned().map(UpdatesEntry::Repo));
    if show_aur_section {
        items.push(UpdatesEntry::Header(format!("AUR ({})", updates.aur.len())));
        items.extend(updates.aur.iter().cloned().map(UpdatesEntry::Aur));
    }
    items
}

fn render_updates_item(entry: &UpdatesEntry, cx: &App) -> AnyElement {
    match entry {
        UpdatesEntry::Header(label) => section_header(label, cx),
        UpdatesEntry::Repo(upgrade) => repo_upgrade_row(upgrade, cx).into_any_element(),
        UpdatesEntry::Aur(candidate) => aur_upgrade_row(candidate, cx).into_any_element(),
    }
}

fn section_header(label: &str, cx: &App) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .text_color(cx.theme().muted_foreground)
        .child(label.to_string())
        .into_any_element()
}

fn repo_upgrade_row(upgrade: &RepoUpgrade, cx: &App) -> Stateful<Div> {
    let muted_fg = cx.theme().muted_foreground;
    let muted_bg = cx.theme().muted;
    div()
        .id(upgrade.name.clone())
        .h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .hover(|s| s.bg(muted_bg.opacity(0.5)))
        .child(div().font_semibold().child(upgrade.name.clone()))
        .child(div().text_color(muted_fg).child(upgrade.repo.clone()))
        .child(
            div()
                .ml_auto()
                .h_flex()
                .items_center()
                .gap_1p5()
                .child(colored_version_delta(&upgrade.old, &upgrade.new, cx))
                .child(
                    div()
                        .text_color(muted_fg)
                        .child(format_bytes(upgrade.download_size)),
                ),
        )
}

pub(crate) fn aur_upgrade_row(candidate: &AurUpgradeCandidate, cx: &App) -> Stateful<Div> {
    let muted_bg = cx.theme().muted;
    div()
        .id(candidate.name.clone())
        .h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .hover(|s| s.bg(muted_bg.opacity(0.5)))
        .child(div().font_semibold().child(candidate.name.clone()))
        .child(div().ml_auto().child(aur_version_delta(candidate, cx)))
}

fn aur_version_delta(candidate: &AurUpgradeCandidate, cx: &App) -> AnyElement {
    let green = cx.theme().green;
    if candidate.remote_version == "latest-commit" {
        let muted_fg = cx.theme().muted_foreground;
        return div()
            .h_flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_color(green)
                    .child(candidate.local_version.clone()),
            )
            .child(Icon::new(PakajoIcon::ArrowRight).text_color(muted_fg))
            .child(div().text_color(green).child("latest-commit"))
            .into_any_element();
    }
    colored_version_delta(&candidate.local_version, &candidate.remote_version, cx)
}

fn colored_version_delta(old: &str, new: &str, cx: &App) -> AnyElement {
    let danger = cx.theme().danger;
    let green = cx.theme().green;
    let muted_fg = cx.theme().muted_foreground;
    let (common, old_suffix, new_suffix) = version_diff(old, new);
    div()
        .h_flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .h_flex()
                .items_baseline()
                .child(div().text_color(muted_fg).child(common.clone()))
                .child(div().text_color(danger).child(old_suffix)),
        )
        .child(Icon::new(PakajoIcon::ArrowRight).text_color(muted_fg))
        .child(
            div()
                .h_flex()
                .items_baseline()
                .child(div().text_color(muted_fg).child(common))
                .child(div().text_color(green).child(new_suffix)),
        )
        .into_any_element()
}
