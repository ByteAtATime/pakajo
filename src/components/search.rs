use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::Alignment;
use cosmic::iced::Color;
use cosmic::iced::Rectangle;
use cosmic::iced::widget::scrollable::{AbsoluteOffset, scroll_by, scroll_to};
use cosmic::widget::rectangle_tracker::{RectangleTracker, RectangleUpdate};
use cosmic::widget::{Column, Row, button, container, scrollable, search_input, space, text};

use pakajo::db::PackageDb;
use pakajo::search::SearchFilter;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

use crate::components::divider::divider;

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
    FilterChanged(SearchFilter),
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
    filter: SearchFilter,
) -> Vec<SearchResult> {
    let (Some(engine), Some(local)) = (search_engine.as_ref(), db.as_ref()) else {
        return Vec::new();
    };
    pakajo::search::dispatch_search(engine, local, &installed, &text, &group_index, filter)
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
    search_input("Search packages", query)
        .id(search_input_id())
        .on_input(|s| crate::Message::Search(SearchMessage::QueryChanged(s)))
        .into()
}

pub fn search_input_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("search-input")
}

fn filter_pill(
    label: &str,
    value: SearchFilter,
    selected: bool,
) -> cosmic::Element<'static, crate::Message> {
    let mut pill = button::custom(text(label.to_string()).size(13))
        .on_press(crate::Message::Search(SearchMessage::FilterChanged(value)))
        .padding([2, 8]);
    if selected {
        pill = pill.class(cosmic::theme::Button::Standard);
    } else {
        pill = pill.class(cosmic::theme::Button::Text);
    }
    pill.into()
}

pub fn search_status_bar<'a>(
    state: SearchState,
    count: usize,
    filter: SearchFilter,
    query: &'a str,
) -> cosmic::Element<'a, crate::Message> {
    let header_text = if state == SearchState::Searching {
        "Searching...".to_string()
    } else if query.trim().is_empty() {
        "Search the repositories".to_string()
    } else {
        format!("{count} results for \"{query}\"")
    };
    Column::new()
        .spacing(8)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([0, 12])
                .push(text(header_text).size(13))
                .push(space::horizontal())
                .push(filter_pill(
                    "All",
                    SearchFilter::All,
                    filter == SearchFilter::All,
                ))
                .push(filter_pill(
                    "Official",
                    SearchFilter::Official,
                    filter == SearchFilter::Official,
                ))
                .push(filter_pill(
                    "AUR",
                    SearchFilter::Aur,
                    filter == SearchFilter::Aur,
                ))
                .push(filter_pill(
                    "Installed",
                    SearchFilter::Installed,
                    filter == SearchFilter::Installed,
                )),
        )
        .into()
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
        if index > 0 {
            list = list.push(divider());
        }
        list = list.push(row);
    }
    list.into()
}

pub fn results_scroller<'a>(
    results: &'a [SearchResult],
    selected_index: Option<usize>,
    scroller: &SelectionScroller,
) -> Element<'a, crate::Message> {
    let scroll = scrollable(results_list(results, selected_index, scroller))
        .direction(cosmic::iced::widget::scrollable::Direction::Vertical(
            cosmic::iced::widget::scrollable::Scrollbar::new()
                .width(4.0)
                .scroller_width(4.0)
                .spacing(0.0),
        ))
        .scrollbar_padding(0)
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

pub fn search_result_row(
    result: &SearchResult,
    index: usize,
    is_selected: bool,
) -> cosmic::Element<'_, crate::Message> {
    let app_theme = cosmic::theme::active();
    let repo = result.repo.as_deref().unwrap_or("aur");
    let is_aur = result.repo.is_none();

    let name_color: Color = if is_selected {
        Color::from(app_theme.cosmic().accent.base)
    } else {
        Color::from(app_theme.cosmic().background(false).on)
    };
    let version_color: Color = if is_aur {
        Color::from(app_theme.cosmic().warning.base)
    } else {
        Color::from(app_theme.cosmic().background(false).component.on)
    };

    let accent_bar = container(
        space::Space::new()
            .width(cosmic::iced::Length::Fixed(3.0))
            .height(cosmic::iced::Length::Fill),
    )
    .class(cosmic::theme::Container::custom(
        move |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(if is_selected {
                Color::from(theme.cosmic().accent.base)
            } else {
                Color::TRANSPARENT
            })),
            ..Default::default()
        },
    ));

    let top_row = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            text(result.name.clone())
                .font(cosmic::font::bold())
                .size(16)
                .class(cosmic::theme::Text::Color(name_color)),
        )
        .push(
            text(result.version.clone())
                .size(13)
                .width(cosmic::iced::Length::Fill)
                .wrapping(cosmic::iced::widget::text::Wrapping::None)
                .ellipsize(cosmic::iced::widget::text::Ellipsize::End(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(cosmic::theme::Text::Color(version_color)),
        )
        .push(space::Space::new().width(cosmic::iced::Length::Fixed(8.0)))
        .push_maybe(result.installed.then(|| {
            cosmic::widget::icon(super::icons::circle_check())
                .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                    |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                        color: Some(theme.cosmic().success.base.into()),
                    },
                )))
                .size(16)
        }))
        .push(
            container(text(repo.to_string()).size(12))
                .padding([2, 6])
                .class(cosmic::theme::Container::custom(
                    move |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                        background: Some(cosmic::iced::Background::Color(if is_aur {
                            Color::from(theme.cosmic().warning.base)
                        } else {
                            Color::from(theme.cosmic().background(false).component.base)
                        })),
                        text_color: Some(if is_aur {
                            Color::from(theme.cosmic().warning.on)
                        } else {
                            Color::from(theme.cosmic().background(false).component.on)
                        }),
                        border: cosmic::iced::Border::default().rounded(4.0_f32),
                        ..Default::default()
                    },
                )),
        );

    let content = Column::new()
        .padding([12, 16])
        .spacing(4)
        .width(cosmic::iced::Length::Fill)
        .push(top_row)
        .push(result.description.as_ref().map(|d| {
            text(d.clone())
                .size(13)
                .class(cosmic::theme::Text::Custom(secondary_text))
                .width(cosmic::iced::Length::Fill)
        }));

    let inner = Row::new().push(accent_bar).push(content);

    button::custom(inner)
        .on_press(crate::Message::Search(SearchMessage::SelectIndex(index)))
        .selected(is_selected)
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .padding(0)
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

                if text.trim().is_empty() {
                    self.results.clear();
                    self.selected_index = None;
                    self.search_state = SearchState::Idle;
                    return Task::none();
                }

                self.begin_search()
            }
            SearchMessage::FilterChanged(filter) => {
                if filter == self.search_filter {
                    return Task::none();
                }
                self.search_filter = filter;
                self.search_seq += 1;
                if self.query.trim().is_empty() {
                    Task::none()
                } else {
                    self.begin_search()
                }
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

    fn begin_search(&mut self) -> cosmic::app::Task<crate::Message> {
        self.search_state = SearchState::Searching;
        let engine = self.search_engine.clone();
        let db = self.db.clone();
        let installed = self.installed_names.clone();
        let group_index = self.group_index.clone();
        let text = self.query.clone();
        let filter = self.search_filter;
        let seq = self.search_seq;
        Task::perform(
            async move { execute_search_for(engine, db, group_index, installed, text, filter) },
            move |results| {
                crate::Message::Search(SearchMessage::ResultsReady { seq, results }).into()
            },
        )
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
