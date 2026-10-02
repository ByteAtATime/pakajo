use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget::{Column, container, text};

use crate::components::theme::{card_style, muted};

pub use crate::background::IndexSyncOutcome;

pub struct LoadingPane;

#[derive(Clone, Debug)]
pub enum LoadingMessage {
    Synced(IndexSyncOutcome),
}

impl LoadingPane {
    pub fn view(&self) -> crate::Element<'static> {
        let spacing = cosmic::theme::spacing();
        let (space_s, space_m, space_l) = (spacing.space_s, spacing.space_m, spacing.space_l);

        let header = Column::new()
            .spacing(space_s)
            .align_x(Alignment::Center)
            .push(text::title2("Building search index").center())
            .push(muted(
                "We're fetching all AUR and repo packages so your searches happen instantly!",
            ));

        let body = container(cosmic::widget::progress_bar::indeterminate_circular().size(32.0))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center);

        let column = Column::new()
            .spacing(space_m)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .push(header)
            .push(body);

        container(
            container(column)
                .width(Length::Fill)
                .max_width(680.0)
                .height(Length::Fill)
                .max_height(480.0)
                .padding(space_l)
                .style(card_style),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.65))),
            ..Default::default()
        })
        .into()
    }
}
