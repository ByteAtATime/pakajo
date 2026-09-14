use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, Space, button, container, divider, scrollable, text, tooltip};

use pakajo::dashboard::{DashboardSnapshot, OptdepEntry, RecentPkg};
use pakajo::utils::{format_bytes, group_thousands};

use crate::Element;
use crate::components::search::SearchMessage;
use crate::components::theme::{card_style, muted};

const MAX_CONTENT_WIDTH: f32 = 840.0;
const BAND_CELL_WIDTH: f32 = 200.0;
const BAND_SKELETON_HEIGHT: f32 = 100.0;
const PAIR_SKELETON_HEIGHT: f32 = 250.0;
const OPTDEP_COUNT_WIDTH: f32 = 90.0;
const AGE_COLUMN_WIDTH: f32 = 90.0;
const OPTDEP_TOOLTIP_ROWS: usize = 8;

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

fn clipped_body(label: String) -> Element<'static> {
    text::body(label)
        .wrapping(cosmic::iced::widget::text::Wrapping::None)
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

fn optdep_popup(entry: &OptdepEntry) -> Element<'static> {
    let mut column = Column::new()
        .spacing(4.0)
        .push(text::body(entry.name.clone()).font(cosmic::font::bold()));
    for (requester, reason) in entry.requesters.iter().take(OPTDEP_TOOLTIP_ROWS) {
        let label = if reason.is_empty() {
            requester.clone()
        } else {
            format!("{requester}: {reason}")
        };
        column = column.push(text::caption(label));
    }
    let hidden = entry.requesters.len().saturating_sub(OPTDEP_TOOLTIP_ROWS);
    if hidden > 0 {
        column = column.push(muted(text::caption(format!("...and {hidden} more"))));
    }
    container(column).into()
}

fn optdep_row(entry: &OptdepEntry) -> Element<'static> {
    let count = entry.requesters.len();
    let count_label = if count == 1 {
        String::from("1 package")
    } else {
        format!("{count} packages")
    };
    let row = Row::new()
        .align_y(Alignment::Center)
        .spacing(12.0)
        .push(container(clipped_body(entry.name.clone())).width(Length::Fill))
        .push(
            container(muted(text::caption(count_label)))
                .width(Length::Fixed(OPTDEP_COUNT_WIDTH))
                .align_x(Alignment::End),
        );
    tooltip(row, optdep_popup(entry), tooltip::Position::FollowCursor)
        .snap_within_viewport(true)
        .class(cosmic::theme::Container::custom(|theme| {
            let mut style = <cosmic::Theme as cosmic::iced::widget::container::Catalog>::style(
                theme,
                &cosmic::theme::Container::Card,
            );
            style.border.width = 1.0;
            style.border.color = theme.cosmic().bg_component_divider().into();
            style.shadow = cosmic::iced::Shadow {
                color: theme.cosmic().shade.into(),
                offset: cosmic::iced::Vector::new(0.0, 4.0),
                blur_radius: 16.0,
            };
            style
        }))
        .into()
}

fn optdep_card(top: &[OptdepEntry], pad: f32) -> Element<'static> {
    let content: Element<'static> = if top.is_empty() {
        muted(text::caption(String::from("No suggestions")))
    } else {
        let mut list = Column::new().spacing(8.0);
        for entry in top {
            list = list.push(optdep_row(entry));
        }
        list.into()
    };
    container(
        Column::new()
            .spacing(8.0)
            .push(text::heading(String::from("Optional dependencies")))
            .push(content),
    )
    .style(card_style)
    .padding(pad)
    .width(Length::Fill)
    .into()
}

fn recent_row(pkg: &RecentPkg) -> Element<'static> {
    let label = Row::new()
        .align_y(Alignment::Center)
        .spacing(8.0)
        .push(text::body(pkg.name.clone()))
        .push(muted(text::caption(format!("({})", pkg.version))));
    let inner = Row::new()
        .align_y(Alignment::Center)
        .spacing(12.0)
        .push(container(label).width(Length::Fill))
        .push(
            container(muted(text::caption(pkg.age.clone())))
                .width(Length::Fixed(AGE_COLUMN_WIDTH))
                .align_x(Alignment::End),
        );
    button::custom(inner)
        .on_press(crate::Message::Search(SearchMessage::QueryChanged(
            pkg.name.clone(),
        )))
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .padding(0)
        .width(Length::Fill)
        .into()
}

fn recent_card(recent: &[RecentPkg], pad: f32) -> Element<'static> {
    let content: Element<'static> = if recent.is_empty() {
        muted(text::caption(String::from("No recent activity")))
    } else {
        let mut list = Column::new().spacing(8.0);
        for pkg in recent {
            list = list.push(recent_row(pkg));
        }
        list.into()
    };
    container(
        Column::new()
            .spacing(8.0)
            .push(text::heading(String::from("Recently updated")))
            .push(content),
    )
    .style(card_style)
    .padding(pad)
    .width(Length::Fill)
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
    let pad = spacing.space_m as f32;
    let pair = match snapshot {
        Some(data) => Row::new()
            .align_y(Alignment::Start)
            .spacing(gap)
            .push(recent_card(&data.recent, pad))
            .push(optdep_card(&data.optdeps, pad)),
        None => Row::new()
            .spacing(gap)
            .push(skeleton_card(PAIR_SKELETON_HEIGHT))
            .push(skeleton_card(PAIR_SKELETON_HEIGHT)),
    };
    let column = Column::new()
        .spacing(gap)
        .width(Length::Fixed(MAX_CONTENT_WIDTH))
        .push(band)
        .push(pair);
    let padded = container(column)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([0.0, side]);
    scrollable(padded).id(crate::page_scroll_id()).into()
}
