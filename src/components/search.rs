use std::collections::HashSet;
use std::sync::Arc;

use cosmic::app::Task;
use cosmic::widget::{Column, Row, button, text, text_input};

use pakajo::db::PackageDb;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

#[derive(Clone, Copy, PartialEq)]
pub enum SearchState {
    Idle,
    Searching,
    Done,
}

#[derive(Clone, Debug)]
pub enum SearchMessage {
    QueryChanged(String),
    ResultsReady {
        seq: u64,
        results: Vec<SearchResult>,
    },
    SelectDelta(i32),
    SelectIndex(usize),
}

pub fn execute_search_for(
    search_engine: Option<Arc<SearchEngine>>,
    db: Option<Arc<PackageDb>>,
    group_index: Arc<Vec<(String, String)>>,
    installed: Arc<HashSet<String>>,
    text: String,
) -> Vec<SearchResult> {
    let (Some(engine), Some(local)) = (search_engine.as_ref(), db.as_ref()) else {
        return Vec::new();
    };
    pakajo::search::dispatch_search(engine, local, &installed, &text, &group_index)
}

pub fn next_selected_index(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    let max = len.checked_sub(1)?;
    let cur = current.unwrap_or(0);
    let next = (cur as i32 + delta).clamp(0, max as i32) as usize;
    if current == Some(next) {
        None
    } else {
        Some(next)
    }
}

pub fn search_bar(query: &str) -> cosmic::Element<'_, crate::Message> {
    text_input("Search packages", query)
        .on_input(|s| crate::Message::Search(SearchMessage::QueryChanged(s)))
        .into()
}

pub fn search_status_text(
    state: SearchState,
    count: usize,
) -> cosmic::Element<'static, crate::Message> {
    let header_text = if state == SearchState::Searching {
        "Searching...".to_string()
    } else {
        format!("{} result(s)", count)
    };
    text(header_text).into()
}

pub fn results_list(
    results: &[SearchResult],
    selected_index: Option<usize>,
) -> cosmic::Element<'_, crate::Message> {
    let mut list = Column::new().spacing(6);
    for (index, result) in results.iter().enumerate() {
        let is_selected = selected_index == Some(index);
        list = list.push(search_result_row(result, index, is_selected));
    }
    list.into()
}

pub fn search_result_row(
    result: &SearchResult,
    index: usize,
    is_selected: bool,
) -> cosmic::Element<'_, crate::Message> {
    let badge = result.repo.as_deref().unwrap_or("aur");

    let top = Row::new()
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .push(text(result.name.clone()).font(cosmic::font::semibold()))
        .push(text(badge.to_string()))
        .push(text(result.version.clone()))
        .push_maybe(result.installed.then(|| text("installed")));

    let content = Column::new()
        .spacing(4)
        .push(top)
        .push_maybe(result.description.as_ref().map(|d| text(d.clone())));

    button::custom(content)
        .on_press(crate::Message::Search(SearchMessage::SelectIndex(index)))
        .selected(is_selected)
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .width(cosmic::iced::Length::Fill)
        .into()
}

impl crate::PakajoApp {
    pub(crate) fn handle_search(
        &mut self,
        message: SearchMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            SearchMessage::QueryChanged(text) => {
                self.query = text.clone();
                self.search_seq = self.search_seq.wrapping_add(1);
                let seq = self.search_seq;

                if text.trim().is_empty() {
                    self.results.clear();
                    self.selected_index = None;
                    self.search_state = SearchState::Idle;
                    return Task::none();
                }

                self.search_state = SearchState::Searching;
                let engine = self.search_engine.clone();
                let db = self.db.clone();
                let installed = self.installed_names.clone();
                let group_index = self.group_index.clone();

                Task::perform(
                    async move { execute_search_for(engine, db, group_index, installed, text) },
                    move |results| {
                        crate::Message::Search(SearchMessage::ResultsReady { seq, results }).into()
                    },
                )
            }
            SearchMessage::ResultsReady { seq, results } => {
                if seq == self.search_seq {
                    self.results = results;
                    self.selected_index = if self.results.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
                    self.search_state = SearchState::Done;
                    if let Some(first) = self.results.first() {
                        return self.load_detail(first.name.clone(), first.source);
                    }
                }
                Task::none()
            }
            SearchMessage::SelectDelta(delta) => {
                if let Some(i) = next_selected_index(self.results.len(), self.selected_index, delta)
                {
                    self.selected_index = Some(i);
                    if let Some(result) = self.results.get(i) {
                        return self.load_detail(result.name.clone(), result.source);
                    }
                }
                Task::none()
            }
            SearchMessage::SelectIndex(i) => {
                if i < self.results.len() {
                    self.selected_index = Some(i);
                    if let Some(result) = self.results.get(i) {
                        return self.load_detail(result.name.clone(), result.source);
                    }
                }
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::next_selected_index;

    #[test]
    fn next_selected_index_empty_returns_none() {
        assert_eq!(next_selected_index(0, None, 1), None);
    }

    #[test]
    fn next_selected_index_clamps_to_bounds() {
        assert_eq!(next_selected_index(5, None, -3), Some(0));
        assert_eq!(next_selected_index(5, None, 100), Some(4));
    }

    #[test]
    fn next_selected_index_noop_returns_none() {
        assert_eq!(next_selected_index(5, Some(4), 1), None);
        assert_eq!(next_selected_index(5, Some(0), -1), None);
    }

    #[test]
    fn next_selected_index_moves() {
        assert_eq!(next_selected_index(5, Some(2), -1), Some(1));
        assert_eq!(next_selected_index(5, Some(2), 1), Some(3));
    }
}
