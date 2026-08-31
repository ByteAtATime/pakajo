use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::Alignment;
use cosmic::iced::Rectangle;
use cosmic::iced::widget::scrollable::{AbsoluteOffset, scroll_by, scroll_to};
use cosmic::widget::rectangle_tracker::{RectangleTracker, RectangleUpdate};
use cosmic::widget::row;
use cosmic::widget::{Column, Row, button, scrollable, space, text, text_input};

use pakajo::db::PackageDb;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ListRect {
    Viewport,
    Row(usize),
}

pub struct SelectionScroller {
    tracker: Option<RectangleTracker<ListRect>>,
    rects: HashMap<ListRect, Rectangle>,
    offset: f32,
}

impl SelectionScroller {
    pub fn new() -> Self {
        SelectionScroller {
            tracker: None,
            rects: HashMap::new(),
            offset: 0.0,
        }
    }

    pub fn track(&mut self, update: RectangleUpdate<ListRect>) {
        match update {
            RectangleUpdate::Init(tracker) => {
                self.tracker = Some(tracker);
            }
            RectangleUpdate::Rectangle((key, rect)) => {
                self.rects.insert(key, rect);
            }
        }
    }

    pub fn scrolled(&mut self, y: f32) {
        self.offset = y;
    }

    pub fn reset_offset(&mut self) {
        self.offset = 0.0;
    }

    pub fn wrap_row<'a>(
        &self,
        index: usize,
        row: impl Into<Element<'a, crate::Message>>,
    ) -> Element<'a, crate::Message> {
        match &self.tracker {
            Some(tracker) => tracker
                .container(ListRect::Row(index), row)
                .width(cosmic::iced::Length::Fill)
                .into(),
            None => row.into(),
        }
    }

    pub fn wrap_viewport<'a>(
        &self,
        scrollable: impl Into<Element<'a, crate::Message>>,
    ) -> Element<'a, crate::Message> {
        match &self.tracker {
            Some(tracker) => tracker
                .container(ListRect::Viewport, scrollable)
                .width(cosmic::iced::Length::Fixed(384.))
                .into(),
            None => scrollable.into(),
        }
    }

    pub fn scroll_to_visible(
        &mut self,
        index: usize,
        fallback_delta: Option<i32>,
    ) -> cosmic::app::Task<crate::Message> {
        let Some(viewport) = self.rects.get(&ListRect::Viewport) else {
            return Task::none();
        };
        if let Some(row) = self.rects.get(&ListRect::Row(index)) {
            if let Some(y) = scroll_offset_for(row, viewport, self.offset) {
                self.offset = y.max(0.0);
                return scroll_to(
                    crate::page_scroll_id(),
                    AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(y),
                    },
                );
            }
            return Task::none();
        }
        match fallback_delta {
            Some(delta) => {
                let prev_signed = (index as i32) - delta;
                let prev_index = if prev_signed < 0 {
                    0
                } else {
                    prev_signed as usize
                };
                if let Some(prev) = self.rects.get(&ListRect::Row(prev_index)) {
                    let pitch = prev.height;
                    let y = delta.signum() as f32 * pitch;
                    self.offset = (self.offset + y).max(0.0);
                    return scroll_by(crate::page_scroll_id(), AbsoluteOffset { x: 0.0, y });
                }
                Task::none()
            }
            None => Task::none(),
        }
    }
}

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
    Rects(RectangleUpdate<ListRect>),
    Scrolled(f32),
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

fn results_list<'a>(
    results: &'a [SearchResult],
    selected_index: Option<usize>,
    scroller: &SelectionScroller,
) -> Element<'a, crate::Message> {
    let mut list = Column::new();
    for (index, result) in results.iter().enumerate() {
        let is_selected = selected_index == Some(index);
        let row = search_result_row(result, index, is_selected);
        let row = scroller.wrap_row(index, row);
        list = list.push(row);
    }
    list.into()
}

pub fn results_scroller<'a>(
    results: &'a [SearchResult],
    selected_index: Option<usize>,
    scroller: &SelectionScroller,
) -> Element<'a, crate::Message> {
    let list = results_list(results, selected_index, scroller);
    let scroll = scrollable(list)
        .scrollbar_width(4)
        .scroller_width(4)
        .width(384.)
        .id(crate::page_scroll_id())
        .on_scroll(|vp| crate::Message::Search(SearchMessage::Scrolled(vp.absolute_offset().y)));
    scroller.wrap_viewport(scroll)
}

pub fn scroll_offset_for(row: &Rectangle, viewport: &Rectangle, offset: f32) -> Option<f32> {
    let content_y = row.y - viewport.y;
    if content_y < offset {
        Some(content_y)
    } else if content_y + row.height > offset + viewport.height {
        Some(content_y + row.height - viewport.height)
    } else {
        None
    }
}

fn secondary_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().background(false).component.on.into()),
        ..cosmic::iced::widget::text::Style::default()
    }
}

fn repo_text(repo: &str) -> cosmic::Element<'_, crate::Message> {
    text(repo.to_string()).font(cosmic::font::semibold()).into()
}

pub fn search_result_row(
    result: &SearchResult,
    index: usize,
    is_selected: bool,
) -> cosmic::Element<'_, crate::Message> {
    let repo = result.repo.as_deref().unwrap_or("aur");

    let top = Row::new()
        .spacing(8)
        .align_y(cosmic::iced::Alignment::End)
        .push(
            text(result.name.clone())
                .font(cosmic::font::semibold())
                .size(16),
        )
        .push(text(result.version.clone()).class(cosmic::theme::Text::Custom(secondary_text)))
        .push(space::Space::new().width(cosmic::iced::Length::Fill))
        .push(
            row![
                result.installed.then(|| {
                    super::icons::circle_check()
                        .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                            |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                                color: Some(theme.cosmic().success.base.into()),
                            },
                        )))
                        .width(16.)
                }),
                repo_text(repo)
            ]
            .align_y(Alignment::Center)
            .spacing(4.),
        );

    let content = Column::new()
        .spacing(4)
        .push(top)
        .push_maybe(result.description.as_ref().map(|d| {
            text(d.clone())
                .size(13)
                .class(cosmic::theme::Text::Custom(secondary_text))
        }));

    button::custom(content)
        .on_press(crate::Message::Search(SearchMessage::SelectIndex(index)))
        .selected(is_selected)
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .padding([8, 16])
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
                    self.scroller.reset_offset();
                    let mut tasks: Vec<cosmic::app::Task<crate::Message>> = Vec::new();
                    if let Some(first) = self.results.first() {
                        tasks.push(self.load_detail(first.name.clone(), first.source));
                    }
                    tasks.push(crate::scroll_to_top());
                    return Task::batch(tasks);
                }
                Task::none()
            }
            SearchMessage::SelectDelta(delta) => {
                if !matches!(self.page, crate::Page::Search)
                    || self.transaction.as_ref().is_some_and(|t| !t.is_checking())
                {
                    return Task::none();
                }
                let Some(i) = next_selected_index(self.results.len(), self.selected_index, delta)
                else {
                    return Task::none();
                };
                self.selected_index = Some(i);
                let detail = match self.results.get(i) {
                    Some(result) => self.load_detail(result.name.clone(), result.source),
                    None => Task::none(),
                };
                let scroll = self.scroller.scroll_to_visible(i, Some(delta));
                Task::batch([detail, scroll])
            }
            SearchMessage::SelectIndex(i) => {
                let Some(result) = self.results.get(i) else {
                    return Task::none();
                };
                self.selected_index = Some(i);
                let detail = self.load_detail(result.name.clone(), result.source);
                let scroll = self.scroller.scroll_to_visible(i, None);
                Task::batch([detail, scroll])
            }
            SearchMessage::Rects(update) => {
                self.scroller.track(update);
                Task::none()
            }
            SearchMessage::Scrolled(y) => {
                self.scroller.scrolled(y);
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn viewport() -> Rectangle {
        Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        }
    }

    fn row(y: f32, height: f32) -> Rectangle {
        Rectangle {
            x: 0.0,
            y,
            width: 100.0,
            height,
        }
    }

    #[test]
    fn scroll_offset_for_bottom_edge_boundary_is_none() {
        assert_eq!(scroll_offset_for(&row(90.0, 20.0), &viewport(), 10.0), None);
    }

    #[test]
    fn scroll_offset_for_partially_below_scrolls_small() {
        assert_eq!(
            scroll_offset_for(&row(95.0, 20.0), &viewport(), 0.0),
            Some(15.0)
        );
    }

    #[test]
    fn scroll_offset_for_above_current_viewport_aligns_top() {
        assert_eq!(
            scroll_offset_for(&row(20.0, 20.0), &viewport(), 50.0),
            Some(20.0)
        );
    }
}
