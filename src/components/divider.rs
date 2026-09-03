use crate::Element;
use cosmic::{
    iced::Color,
    widget::{container, text},
};

pub fn divider() -> Element<'static> {
    // TODO: is this the proper way to do this?
    container(text(""))
        .width(cosmic::iced::Length::Fill)
        .height(1.0)
        .style(
            |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(Color::from(
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
        .height(cosmic::iced::Length::Fill)
        .style(
            |theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(Color::from(
                    theme.cosmic().background(false).divider,
                ))),
                ..Default::default()
            },
        )
        .into()
}
