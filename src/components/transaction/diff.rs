use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{rich_text, span};
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, text};
use pakajo::diff::{RenderedLine, StyledSegment};

use super::shared::{BadgeColor, destructive_color, muted_color, success_color};
use crate::Element;
use crate::components::theme::muted;

type MonoSpan = cosmic::iced::widget::text::Span<'static, (), cosmic::font::Font>;

fn segment_color(segment: &StyledSegment) -> Color {
    let color = segment.color;
    Color::from_rgba8(color.r, color.g, color.b, f32::from(color.a) / 255.0)
}

fn marker_color(theme: &cosmic::Theme, marker: char) -> Color {
    match marker {
        '+' => Color::from(theme.cosmic().success.base),
        '-' => Color::from(theme.cosmic().destructive.base),
        _ => muted_color(theme),
    }
}

fn code_spans(
    segments: &[StyledSegment],
    fallback: &str,
    emphasis: Option<Color>,
) -> Vec<MonoSpan> {
    if segments.is_empty() {
        return vec![
            span(fallback.to_owned())
                .font(cosmic::font::mono())
                .to_static(),
        ];
    }
    segments
        .iter()
        .map(|segment| {
            let font = cosmic::font::Font {
                weight: if segment.emphasized {
                    cosmic::iced::font::Weight::Bold
                } else {
                    cosmic::iced::font::Weight::Normal
                },
                ..cosmic::font::mono()
            };
            let current = span(segment.text.clone())
                .color(segment_color(segment))
                .font(font);
            match (segment.emphasized, emphasis) {
                (true, Some(background)) => current
                    .background(Background::Color(background))
                    .to_static(),
                _ => current.to_static(),
            }
        })
        .collect()
}

fn code_text(spans: Vec<MonoSpan>) -> Element<'static> {
    container(
        rich_text(spans)
            .font(cosmic::font::mono())
            .size(14.0)
            .line_height(LineHeight::Absolute(20.0.into())),
    )
    .width(Length::Fill)
    .into()
}

fn diff_row(
    marker: char,
    body: &str,
    segments: &[StyledSegment],
    background: Option<Color>,
) -> Element<'static> {
    let marker_cell = container(text::monotext(marker.to_string()))
        .width(Length::Fixed(16.0))
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(marker_color(theme, marker)),
            ..Default::default()
        });
    Row::new()
        .spacing(8)
        .push(marker_cell)
        .push(code_text(code_spans(segments, body, background)))
        .into()
}

fn count_chip(label: String, color: BadgeColor) -> Element<'static> {
    container(text(label))
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(color(theme)),
            ..Default::default()
        })
        .into()
}

fn file_header_element(name: String, added: usize, removed: usize) -> Element<'static> {
    let chip = container(text(name))
        .padding([2.0, 10.0])
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.16,
                ..muted_color(theme)
            })),
            border: Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let header = Row::new()
        .spacing(8)
        .push(chip)
        .push_maybe((added > 0).then(|| count_chip(format!("+{added}"), success_color)))
        .push_maybe((removed > 0).then(|| count_chip(format!("-{removed}"), destructive_color)));
    container(header)
        .padding([10.0, 0.0, 4.0, 0.0])
        .width(Length::Fill)
        .into()
}

fn gap_element(elided: usize) -> Element<'static> {
    muted(
        text(format!("<{} lines unchanged>", elided))
            .center()
            .width(Length::Fill),
    )
}

pub(crate) fn diff_rows_column(rows: &[RenderedLine], is_new: bool) -> Element<'_> {
    let theme = cosmic::theme::active();
    let success = success_color(&theme);
    let destructive = destructive_color(&theme);
    let mut column = Column::new().spacing(0);
    for rendered in rows {
        let row = match rendered {
            RenderedLine::FileHeader {
                name,
                added,
                removed,
            } => file_header_element(name.clone(), *added, *removed),
            RenderedLine::Gap { elided } => gap_element(*elided),
            RenderedLine::Code {
                marker,
                text,
                segments,
            } => {
                if is_new {
                    code_text(code_spans(segments, text, None))
                } else {
                    let background = match marker {
                        '+' => Some(Color { a: 0.3, ..success }),
                        '-' => Some(Color {
                            a: 0.3,
                            ..destructive
                        }),
                        _ => None,
                    };
                    diff_row(*marker, text, segments, background)
                }
            }
        };
        column = column.push(row);
    }
    column.into()
}
