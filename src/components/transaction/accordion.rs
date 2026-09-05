use super::TransactionMessage;
use super::shared::{accent_color, destructive_color, muted, muted_color, on_color, tinted};
use super::state::StageState;
use crate::Element;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Background, Border, Color, Length, Shadow};
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
    let mut panels = Column::new().spacing(6);
    for (i, (section, expanded)) in sections.into_iter().enumerate() {
        panels = panels.push(stage_row(section, expanded, i));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if finished {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

pub(super) fn stage_row(section: Section<'_>, expanded: bool, index: usize) -> Element<'_> {
    let Section {
        label,
        state,
        content,
        header_suffix,
    } = section;
    let gutter = stage_glyph(state);
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

    let body = Row::new()
        .align_y(Vertical::Top)
        .push(gutter)
        .push(container(content).width(Length::Fill));

    container(body)
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(move |theme: &cosmic::Theme| stage_panel_style(theme, state))
        .into()
}

const GLYPH_GUTTER_WIDTH: f32 = 18.0;

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
    let (glyph_text, glyph_color_fn): (&'static str, fn(&cosmic::Theme) -> Color) = match state {
        StageState::Done => ("✓", accent_color),
        StageState::Active => ("●", accent_color),
        StageState::Pending => ("○", muted_color),
        StageState::Failed => ("✗", destructive_color),
    };
    container(tinted(text(glyph_text), glyph_color_fn))
        .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .into()
}

fn header_row<'a>(
    state: StageState,
    label: &'static str,
    suffix: Option<Element<'a>>,
) -> Element<'a> {
    let label_widget = match state {
        StageState::Active => text(label).font(cosmic::font::bold()).size(18.0),
        StageState::Done => text(label).font(cosmic::font::semibold()),
        _ => text(label),
    };
    let label_color_fn: fn(&cosmic::Theme) -> Color = match state {
        StageState::Done => on_color,
        StageState::Active => accent_color,
        StageState::Pending => muted_color,
        StageState::Failed => on_color,
    };

    let mut row = Row::new()
        .width(Length::Fill)
        .spacing(8)
        .push(tinted(label_widget, label_color_fn))
        .push(space::horizontal());
    if let Some(suffix) = suffix {
        row = row.push(suffix);
    }
    row.into()
}

fn stage_panel_style(theme: &cosmic::Theme, state: StageState) -> container::Style {
    let cosmic = theme.cosmic();
    let surface_base = Color::from(cosmic.background(false).base);
    let surface_mid = Color::from(cosmic.background(false).small_widget);
    let surface_high = Color::from(cosmic.background(false).component.base);
    let divider = Color::from(cosmic.background(false).divider);
    let on = Color::from(cosmic.background(false).on);

    let accent = Color::from(cosmic.accent.base);
    let destructive = Color::from(cosmic.destructive.base);
    let (background, border_color, text_color) = match state {
        StageState::Active => (surface_base, accent, on),
        StageState::Failed => (surface_base, destructive, on),
        StageState::Done => (surface_high, divider, on),
        StageState::Pending => (
            Color {
                a: 0.35,
                ..surface_mid
            },
            Color { a: 0.4, ..divider },
            Color { a: 0.5, ..on },
        ),
    };
    let mut style = container::Style {
        text_color: Some(text_color),
        background: Some(Background::Color(background)),
        border: Border {
            radius: cosmic.corner_radii.radius_s.into(),
            width: if matches!(state, StageState::Active | StageState::Failed) {
                2.0
            } else {
                1.0
            },
            color: border_color,
        },
        ..Default::default()
    };
    if state == StageState::Active {
        style.shadow = Shadow {
            color: Color { a: 0.10, ..accent },
            offset: Default::default(),
            blur_radius: 15.0,
        };
    }
    style
}
