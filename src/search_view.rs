use crate::search::SearchResult;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{ActiveTheme as _, StyledExt as _};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SearchState {
    Idle,
    Searching,
    Done,
}

pub(crate) struct SearchView {
    results: Vec<SearchResult>,
    selected_index: Option<usize>,
}

impl SearchView {
    pub(crate) fn new() -> Self {
        Self {
            results: Vec::new(),
            selected_index: None,
        }
    }

    pub(crate) fn set_results(&mut self, results: Vec<SearchResult>) {
        self.results = results;
        self.selected_index = if self.results.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    pub(crate) fn set_selected_index(&mut self, index: usize) {
        self.selected_index = Some(index);
    }

    #[allow(dead_code)]
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub(crate) fn result_at(&self, index: usize) -> Option<&SearchResult> {
        self.results.get(index)
    }

    pub(crate) fn selected_name(&self) -> Option<&str> {
        self.selected_index
            .and_then(|i| self.results.get(i))
            .map(|r| r.name.as_str())
    }

    pub(crate) fn first_result(&self) -> Option<&SearchResult> {
        self.results.first()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.results.clear();
        self.selected_index = None;
    }

    pub(crate) fn render(
        &self,
        search_state: SearchState,
        on_select: Arc<dyn Fn(usize, &mut App)>,
        cx: &App,
    ) -> AnyElement {
        let header = if matches!(search_state, SearchState::Searching) {
            "Searching…".to_string()
        } else {
            format!("{} result(s)", self.results.len())
        };

        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.results.len());
        for (index, result) in self.results.iter().enumerate() {
            rows.push(
                self.result_row(index, result, on_select.clone(), cx)
                    .into_any_element(),
            );
        }

        div()
            .w(rems(24.))
            .flex_shrink_0()
            .h_full()
            .v_flex()
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
                    .v_flex()
                    .children(rows),
            )
            .into_any_element()
    }

    fn result_row(
        &self,
        index: usize,
        result: &SearchResult,
        on_select: Arc<dyn Fn(usize, &mut App)>,
        cx: &App,
    ) -> impl IntoElement {
        let is_selected = self.selected_index == Some(index);
        let badge = result.repo.as_deref().unwrap_or("aur");
        let accent = cx.theme().accent;
        let muted = cx.theme().muted_foreground;
        div()
            .id(result.name.clone())
            .v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .when(is_selected, |row| row.bg(accent.opacity(0.12)))
            .hover(|s| s.bg(accent.opacity(0.06)))
            .on_click(move |_, _, cx| on_select(index, cx))
            .child(
                div()
                    .h_flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().font_semibold().child(result.name.clone()))
                    .child(
                        div()
                            .text_color(muted)
                            .text_size(rems(0.75))
                            .child(badge.to_string()),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_color(muted)
                            .text_size(rems(0.75))
                            .child(result.version.clone()),
                    ),
            )
            .children(result.description.clone().map(|d| {
                div()
                    .w_full()
                    .truncate()
                    .text_color(muted)
                    .text_size(rems(0.8))
                    .child(d)
            }))
    }
}

pub(crate) fn centered() -> Div {
    div().flex_1().size_full().items_center().justify_center()
}
