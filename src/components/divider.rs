use cosmic::{
    iced::Color,
    widget::{container, text},
};

pub fn divider() -> cosmic::Element<'static, crate::Message> {
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
