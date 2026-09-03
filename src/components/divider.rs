use crate::Element;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget::{container, text};

pub fn divider() -> Element<'static> {
    container(text(""))
        .width(Length::Fill)
        .height(1.0)
        .style(
            |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                background: Some(Background::Color(Color::from(
                    theme.cosmic().background(false).divider,
                ))),
                ..Default::default()
            },
        )
        .into()
}

pub fn vdivider() -> Element<'static> {
    container(text(""))
        .width(1.0)
        .height(Length::Fill)
        .style(
            |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                background: Some(Background::Color(Color::from(
                    theme.cosmic().background(false).divider,
                ))),
                ..Default::default()
            },
        )
        .into()
}
