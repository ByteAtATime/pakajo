use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, Space, container, divider, scrollable, text};

use pakajo::dashboard::DashboardSnapshot;
use pakajo::utils::{format_bytes, group_thousands};

use crate::Element;
use crate::components::theme::{card_style, muted};

const MAX_CONTENT_WIDTH: f32 = 840.0;
const BAND_CELL_WIDTH: f32 = 200.0;
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

fn band_stat(label: String, value: String, sub: String) -> Element<'static> {
    container(
        Column::new()
            .align_x(Alignment::Center)
            .spacing(2.0)
            .push(muted(text::caption(label)))
            .push(text::title3(value))
            .push(muted(text::caption(sub))),
    )
    .width(Length::Fixed(BAND_CELL_WIDTH))
    .align_x(Alignment::Center)
    .into()
}

fn live_band(snapshot: &DashboardSnapshot) -> Element<'static> {
    let gap = cosmic::theme::spacing().space_xs as f32;
    let packages_sub = format!(
        "{} repo \u{b7} {} AUR",
        group_thousands(snapshot.repo_count as i64),
        group_thousands(snapshot.aur_count as i64)
    );
    let disk_sub = format!(
        "{} repo \u{b7} {} AUR",
        format_bytes(snapshot.repo_bytes),
        format_bytes(snapshot.aur_bytes)
    );
    let row = Row::new()
        .align_y(Alignment::Center)
        .spacing(gap)
        .width(Length::Shrink)
        .push(band_stat(
            String::from("Packages"),
            group_thousands(snapshot.installed_total as i64),
            packages_sub,
        ))
        .push(divider::vertical::default())
        .push(band_stat(
            String::from("Size on disk"),
            format_bytes(snapshot.total_bytes),
            disk_sub,
        ));
    container(row)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}

pub fn dashboard_view(snapshot: Option<&DashboardSnapshot>) -> Element<'static> {
    let spacing = cosmic::theme::spacing();
    let gap = spacing.space_xs as f32;
    let side = spacing.space_m as f32;
    let band = match snapshot {
        Some(data) => live_band(data),
        None => skeleton_card(BAND_SKELETON_HEIGHT),
    };
    let pair = Row::new()
        .spacing(gap)
        .push(skeleton_card(PACKAGE_SKELETON_HEIGHT))
        .push(skeleton_card(PACKAGE_SKELETON_HEIGHT));
    let column = Column::new()
        .spacing(gap)
        .width(Length::Fixed(MAX_CONTENT_WIDTH))
        .push(band)
        .push(pair)
        .push(skeleton_card(NEWS_SKELETON_HEIGHT));
    let padded = container(column)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([0.0, side]);
    scrollable(padded).id(crate::page_scroll_id()).into()
}
