use cosmic::iced::widget::progress_bar;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{button, container, text};

use crate::Element;

type Tint = fn(&cosmic::Theme) -> Color;

pub(crate) fn tinted_button(
    tint: Tint,
    alpha: f32,
) -> Box<dyn Fn(bool, &cosmic::Theme) -> button::Style> {
    Box::new(move |_, theme| button::Style {
        text_color: Some(Color {
            a: alpha,
            ..tint(theme)
        }),
        ..Default::default()
    })
}

pub(crate) fn tinted_button_disabled(
    tint: Tint,
    alpha: f32,
) -> Box<dyn Fn(&cosmic::Theme) -> button::Style> {
    Box::new(move |theme| button::Style {
        text_color: Some(Color {
            a: alpha,
            ..tint(theme)
        }),
        ..Default::default()
    })
}

pub(crate) fn mono_text(name: &str) -> Element<'static> {
    tinted(text::monotext(name.to_string()), on_color)
}

pub(crate) fn thin_bar(value: f32) -> Element<'static> {
    progress_bar(0.0..=100.0, value)
        .length(Length::Fill)
        .girth(6.0)
        .into()
}

pub(crate) fn muted<'a>(content: impl Into<Element<'a>>) -> Element<'a> {
    container(content)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(muted_color(t)),
            ..Default::default()
        })
        .into()
}

pub(crate) fn tinted<'a>(
    content: impl Into<Element<'a>>,
    color_fn: fn(&cosmic::Theme) -> Color,
) -> Element<'a> {
    container(content.into())
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(color_fn(theme)),
            ..Default::default()
        })
        .into()
}

pub(crate) fn muted_color(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    Color { a: 0.7, ..on }
}

pub(crate) fn on_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().background(false).on)
}

pub(crate) fn accent_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().accent.base)
}

pub(crate) fn success_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().success.base)
}

pub(crate) fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

pub(crate) fn warning_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().warning.base)
}

pub(crate) fn pill(
    label: impl Into<String>,
    color_fn: fn(&cosmic::Theme) -> Color,
) -> Element<'static> {
    container(text(label.into()))
        .padding([2.0, 8.0])
        .style(move |theme: &cosmic::Theme| {
            let colored = color_fn(theme);
            container::Style {
                text_color: Some(colored),
                background: Some(Background::Color(Color { a: 0.10, ..colored })),
                border: Border {
                    radius: theme.cosmic().corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

pub(crate) fn muted_text_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    let mut on = Color::from(theme.cosmic().background(false).on);
    on.a = 0.7;
    cosmic::iced::widget::text::Style {
        color: Some(on),
        ..Default::default()
    }
}

pub(crate) fn themed_text<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    style_fn: fn(&cosmic::Theme) -> cosmic::iced::widget::text::Style,
) -> Element<'a> {
    text(content)
        .class(cosmic::theme::Text::Custom(style_fn))
        .into()
}

pub(crate) fn themed_mono_text<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    style_fn: fn(&cosmic::Theme) -> cosmic::iced::widget::text::Style,
) -> Element<'a> {
    text::monotext(content)
        .class(cosmic::theme::Text::Custom(style_fn))
        .into()
}

pub(crate) fn muted_text<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a> {
    themed_text(content, muted_text_style)
}

pub(crate) fn muted_mono<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a> {
    themed_mono_text(content, muted_text_style)
}
