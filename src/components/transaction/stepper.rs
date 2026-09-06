use super::TransactionMessage;
use super::shared::{accent_color, destructive_color, muted, muted_color, on_color, tinted};
use super::state::StageState;
use crate::Element;
use crate::components::icons;
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::border::Radius;
use cosmic::iced::widget::Stack;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::divider;
use cosmic::widget::{Column, Row, button, container, scrollable, space, text};

pub(super) fn action_footer() -> Element<'static> {
    let header_divider = divider::horizontal::default();
    let close =
        button::standard("Close").on_press(crate::Message::Transaction(TransactionMessage::Close));
    Column::new()
        .spacing(12)
        .push(header_divider)
        .push(Row::new().push(space::horizontal()).push(close))
        .into()
}

pub(super) struct Section<'a> {
    pub(super) label: &'static str,
    pub(super) state: StageState,
    pub(super) content: Option<Element<'a>>,
    pub(super) header_suffix: Option<Element<'a>>,
}

impl<'a> Section<'a> {
    pub(super) fn new(label: &'static str, state: StageState) -> Self {
        Self {
            label,
            state,
            content: None,
            header_suffix: None,
        }
    }
}

pub(super) fn sections_view(
    title: String,
    sections: Vec<(Section<'_>, bool)>,
    finished: bool,
) -> Element<'_> {
    let count = sections.len();
    let states: Vec<StageState> = sections.iter().map(|(s, _)| s.state).collect();
    let mut panels = Column::new();
    for (i, (section, expanded)) in sections.into_iter().enumerate() {
        let prev_state = if i > 0 { Some(states[i - 1]) } else { None };
        panels = panels.push(stage_row(section, expanded, i, prev_state, i + 1 < count));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if finished {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

pub(super) fn stage_row(
    section: Section<'_>,
    expanded: bool,
    index: usize,
    prev_state: Option<StageState>,
    has_next: bool,
) -> Element<'_> {
    let Section {
        label,
        state,
        content,
        header_suffix,
    } = section;
    let gutter = stage_gutter(state, prev_state, has_next);
    let has_suffix = header_suffix.is_some();
    let header = header_row(state, label, header_suffix);

    let content: Element<'_> = match state {
        StageState::Pending | StageState::Active => match content {
            Some(content) => Column::new()
                .spacing(6)
                .push(header)
                .push(muted(content))
                .into(),
            None => header,
        },
        StageState::Failed => {
            let mut col = Column::new()
                .spacing(6)
                .push(header)
                .push(muted(text("failed")));
            if let Some(detail) = content {
                col = col.push(detail);
            }
            col.into()
        }
        StageState::Done => match content {
            Some(detail) if expanded || !has_suffix => toggle_detail(header, index, detail),
            Some(_) => done_toggle(header, index),
            None => header,
        },
    };

    Row::new()
        .align_y(Vertical::Top)
        .spacing(8)
        .push(gutter)
        .push(
            container(content)
                .width(Length::Fill)
                .padding([0.0, 0.0, STEP_GAP, 0.0]),
        )
        .into()
}

const GLYPH_GUTTER_WIDTH: f32 = 18.0;
const TITLE_LINE_HEIGHT: f32 = 30.0;
const CONNECTOR_WIDTH: f32 = 2.0;
const STEP_GAP: f32 = 14.0;

fn stage_gutter(
    state: StageState,
    prev_state: Option<StageState>,
    has_next: bool,
) -> Element<'static> {
    let mut stack = Stack::new();
    if let Some(prev) = prev_state {
        stack = stack.push(rail_incoming(prev));
    }
    if has_next {
        stack = stack.push(rail_outgoing(state));
    }
    stack
        .push(node_backing())
        .push(stage_glyph(state))
        .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .height(Length::Fill)
        .into()
}

fn rail_line(height: Length, state: StageState) -> Element<'static> {
    container(text(""))
        .width(Length::Fixed(CONNECTOR_WIDTH))
        .height(height)
        .style(move |theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            let color = match state {
                StageState::Done => Color::from(cosmic.accent.base),
                _ => Color::from(cosmic.background(false).divider),
            };
            container::Style {
                background: Some(Background::Color(color)),
                ..Default::default()
            }
        })
        .into()
}

fn rail_incoming(prev: StageState) -> Element<'static> {
    container(rail_line(Length::Fill, prev))
        .width(Length::Fill)
        .height(Length::Fixed(TITLE_LINE_HEIGHT / 2.0))
        .align_x(Horizontal::Center)
        .into()
}

fn rail_outgoing(state: StageState) -> Element<'static> {
    container(rail_line(Length::Fill, state))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([TITLE_LINE_HEIGHT / 2.0, 0.0, 0.0, 0.0])
        .align_x(Horizontal::Center)
        .into()
}

fn node_backing() -> Element<'static> {
    let disc = container(text(""))
        .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .height(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .style(|theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            container::Style {
                background: Some(Background::Color(Color::from(
                    cosmic.background(false).base,
                ))),
                border: Border {
                    radius: Radius::from(GLYPH_GUTTER_WIDTH / 2.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
    container(disc)
        .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .height(Length::Fixed(TITLE_LINE_HEIGHT))
        .align_y(Vertical::Center)
        .into()
}

fn done_toggle(header: Element<'_>, index: usize) -> Element<'_> {
    toggle_button(header, TransactionMessage::ToggleStage(index))
}

pub(super) fn toggle_button<'a>(
    header: impl Into<Element<'a>>,
    message: TransactionMessage,
) -> Element<'a> {
    button::custom(header.into())
        .padding([2.0, 0.0])
        .width(Length::Fill)
        .class(cosmic::theme::Button::Transparent)
        .on_press(crate::Message::Transaction(message))
        .into()
}

fn toggle_detail<'a>(header: Element<'a>, index: usize, detail: Element<'a>) -> Element<'a> {
    Column::new()
        .spacing(6)
        .push(done_toggle(header, index))
        .push(detail)
        .into()
}

fn stage_glyph(state: StageState) -> Element<'static> {
    let (glyph, glyph_color_fn): (cosmic::widget::icon::Handle, fn(&cosmic::Theme) -> Color) =
        match state {
            StageState::Done => (icons::circle_check(), accent_color),
            StageState::Active => (icons::circle_dot(), accent_color),
            StageState::Pending => (icons::circle(), muted_color),
            StageState::Failed => (icons::circle_x(), destructive_color),
        };
    container(
        cosmic::widget::icon(glyph)
            .size(18)
            .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                move |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                    color: Some(glyph_color_fn(theme)),
                },
            ))),
    )
    .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
    .height(Length::Fixed(TITLE_LINE_HEIGHT))
    .align_y(Vertical::Center)
    .into()
}

fn header_row<'a>(
    state: StageState,
    label: &'static str,
    suffix: Option<Element<'a>>,
) -> Element<'a> {
    let label_widget = text::title4(label);
    let label_color_fn: fn(&cosmic::Theme) -> Color = match state {
        StageState::Done => on_color,
        StageState::Active => accent_color,
        StageState::Pending => muted_color,
        StageState::Failed => on_color,
    };

    let mut row = Row::new()
        .width(Length::Fill)
        .align_y(Vertical::Center)
        .spacing(8)
        .push(tinted(label_widget, label_color_fn))
        .push(space::horizontal());
    if let Some(suffix) = suffix {
        row = row.push(suffix);
    }
    row.into()
}
