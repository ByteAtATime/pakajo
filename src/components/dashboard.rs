use cosmic::iced::Length;
use cosmic::widget::{Column, Row, Space, container, scrollable};

use crate::Element;
use crate::components::theme::card_style;

const MAX_CONTENT_WIDTH: f32 = 840.0;
const BAND_SKELETON_HEIGHT: f32 = 100.0;
const PACKAGE_SKELETON_HEIGHT: f32 = 250.0;
const NEWS_SKELETON_HEIGHT: f32 = 150.0;

fn skeleton_card(height: f32) -> Element<'static> {
    container(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(height)),
    )
    .width(Length::Fill)
    .style(card_style)
    .into()
}

pub fn dashboard_view() -> Element<'static> {
    let spacing = cosmic::theme::spacing();
    let gap = spacing.space_xs as f32;
    let side = spacing.space_s as f32;
    let pair = Row::new()
        .spacing(gap)
        .push(skeleton_card(PACKAGE_SKELETON_HEIGHT))
        .push(skeleton_card(PACKAGE_SKELETON_HEIGHT));
    let column = Column::new()
        .spacing(gap)
        .width(Length::Fixed(MAX_CONTENT_WIDTH))
        .push(skeleton_card(BAND_SKELETON_HEIGHT))
        .push(pair)
        .push(skeleton_card(NEWS_SKELETON_HEIGHT));
    let padded = container(column)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([0.0, side]);
    scrollable(padded).id(crate::page_scroll_id()).into()
}
