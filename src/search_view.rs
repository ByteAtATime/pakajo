use crate::icon::PakajoIcon;
use crate::search::SearchResult;
use crate::session::SearchState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{ActiveTheme as _, Icon, StyledExt as _};
use std::sync::Arc;

pub(crate) struct SearchView {
    scroll_handle: ScrollHandle,
}

impl SearchView {
    pub(crate) fn new() -> Self {
        Self {
            scroll_handle: ScrollHandle::default(),
        }
    }

    pub(crate) fn scroll_to(&self, index: usize) {
        self.scroll_handle.scroll_to_item(index);
    }

    pub(crate) fn render(
        &self,
        results: &[SearchResult],
        selected_index: Option<usize>,
        search_state: SearchState,
        on_select: Arc<dyn Fn(usize, &mut App)>,
        cx: &App,
    ) -> AnyElement {
        let header = if matches!(search_state, SearchState::Searching) {
            "Searching…".to_string()
        } else {
            format!("{} result(s)", results.len())
        };

        let mut rows: Vec<AnyElement> = Vec::with_capacity(results.len());
        for (index, result) in results.iter().enumerate() {
            rows.push(
                self.result_row(index, result, selected_index, on_select.clone(), cx)
                    .into_any_element(),
            );
        }

        div()
            .w(rems(24.))
            .flex_shrink_0()
            .v_flex()
            .h_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(header),
            )
            .child(
                div()
                    .id("results")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .v_flex()
                    .children(rows),
            )
            .into_any_element()
    }

    fn result_row(
        &self,
        index: usize,
        result: &SearchResult,
        selected_index: Option<usize>,
        on_select: Arc<dyn Fn(usize, &mut App)>,
        cx: &App,
    ) -> impl IntoElement {
        let is_selected = selected_index == Some(index);
        let badge = result.repo.as_deref().unwrap_or("aur");
        let muted_fg = cx.theme().muted_foreground;
        let muted_bg = cx.theme().muted;
        div()
            .id(result.name.clone())
            .v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .when(is_selected, |row| row.bg(muted_bg))
            .hover(|s| s.bg(muted_bg.opacity(0.5)))
            .on_click(move |_, _, cx| on_select(index, cx))
            .child(
                div()
                    .h_flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().font_semibold().child(result.name.clone()))
                    .child(
                        div()
                            .text_color(muted_fg)
                            .text_size(rems(0.75))
                            .child(badge.to_string()),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_color(muted_fg)
                                    .text_size(rems(0.75))
                                    .child(result.version.clone()),
                            )
                            .children(result.installed.then(|| {
                                Icon::new(PakajoIcon::CircleCheck)
                                    .text_color(cx.theme().green)
                                    .size(rems(0.75))
                            })),
                    ),
            )
            .children(result.description.clone().map(|d| {
                div()
                    .w_full()
                    .truncate()
                    .text_color(muted_fg)
                    .text_size(rems(0.8))
                    .child(d)
            }))
    }
}

pub(crate) fn centered() -> Div {
    div().flex_1().size_full().items_center().justify_center()
}
