use cosmic::iced::core::text::EllipsizeHeightLimit;
use cosmic::iced::{Alignment, Color, Length};
use cosmic::widget::{Column, Row, button, container, divider, scrollable, text, tooltip};

use pakajo::dashboard::{DashboardMessage, DashboardSnapshot, OptdepEntry, RecentPkg};
use pakajo::utils::{format_bytes, group_thousands};

use cosmic::app::Task;

use crate::Element;
use crate::PakajoCtx;
use crate::components::search::SearchMessage;
use crate::components::task::blocking_task;

#[derive(Default)]
pub struct DashboardState {
    pub(crate) snapshot: Option<DashboardSnapshot>,
    pub(crate) seq: u64,
}

impl DashboardState {
    pub fn refresh(&mut self) -> Task<crate::Message> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        blocking_task(
            pakajo::dashboard::gather_dashboard,
            "dashboard refresh cancelled",
            move |result| match result {
                Ok((foreign, snapshot)) => {
                    crate::Message::Dashboard(DashboardMessage::SnapshotReady {
                        seq,
                        foreign,
                        snapshot,
                    })
                    .into()
                }
                Err(error) => {
                    crate::Message::Dashboard(DashboardMessage::LoadFailed { seq, error }).into()
                }
            },
        )
    }

    pub fn update(
        &mut self,
        message: DashboardMessage,
        ctx: &mut PakajoCtx,
    ) -> Task<crate::Message> {
        match message {
            DashboardMessage::SnapshotReady {
                seq,
                foreign,
                snapshot,
            } => {
                if seq != self.seq {
                    return Task::none();
                }
                ctx.foreign_names = std::sync::Arc::new(foreign);
                self.snapshot = Some(snapshot);
                Task::none()
            }
            DashboardMessage::LoadFailed { .. } => Task::none(),
        }
    }
}
use crate::components::theme::{card_style, muted};

const MAX_CONTENT_WIDTH: f32 = 840.0;
const BAND_CELL_WIDTH: f32 = 200.0;
const TRAILING_COLUMN_WIDTH: f32 = 90.0;
const OPTDEP_TOOLTIP_ROWS: usize = 8;
const CARD_ROWS: usize = 5;

fn hidden<'a>(content: impl Into<Element<'a>>) -> Element<'a> {
    container(content)
        .style(|_: &cosmic::Theme| container::Style {
            text_color: Some(Color::TRANSPARENT),
            ..Default::default()
        })
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

fn ellipsized_body(label: String, width: Length) -> Element<'static> {
    text::body(label)
        .width(width)
        .wrapping(cosmic::iced::widget::text::Wrapping::None)
        .ellipsize(cosmic::iced::widget::text::Ellipsize::End(
            EllipsizeHeightLimit::Lines(1),
        ))
        .into()
}

fn clipped_body(label: String) -> Element<'static> {
    ellipsized_body(label, Length::Fill)
}

fn searchable_row(name: String, leading: Element<'static>, trailing: String) -> Element<'static> {
    let inner = Row::new()
        .align_y(Alignment::Center)
        .spacing(12.0)
        .push(container(leading).width(Length::Fill))
        .push(
            container(muted(text::caption(trailing)))
                .width(Length::Fixed(TRAILING_COLUMN_WIDTH))
                .align_x(Alignment::End),
        );
    button::custom(inner)
        .on_press(crate::Message::Search(SearchMessage::QueryChanged(name)))
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .padding(0)
        .width(Length::Fill)
        .into()
}

fn list_card(
    title: String,
    empty_label: String,
    rows: Vec<Element<'static>>,
    pad: f32,
) -> Element<'static> {
    let content: Element<'static> = if rows.is_empty() {
        muted(text::caption(empty_label))
    } else {
        let mut list = Column::new();
        for row in rows {
            list = list.push(container(row).padding([4.0, 0.0]));
        }
        list.into()
    };
    container(
        Column::new()
            .spacing(8.0)
            .push(text::heading(title))
            .push(content),
    )
    .style(card_style)
    .padding(pad)
    .width(Length::Fill)
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

fn skeleton_band() -> Element<'static> {
    Column::new()
        .spacing(2.0)
        .push(hidden(text::caption(String::from("Packages"))))
        .push(hidden(text::title3(String::from("0"))))
        .push(hidden(text::caption(String::from("0 repo \u{b7} 0 AUR"))))
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
    let column = column
        .push_maybe((hidden > 0).then(|| muted(text::caption(format!("...and {hidden} more")))));
    container(column).into()
}

fn optdep_row(entry: &OptdepEntry) -> Element<'static> {
    let count = entry.requesters.len();
    let count_label = if count == 1 {
        String::from("1 package")
    } else {
        format!("{count} packages")
    };
    let pressed = searchable_row(
        entry.name.clone(),
        clipped_body(entry.name.clone()),
        count_label,
    );
    tooltip(
        pressed,
        optdep_popup(entry),
        tooltip::Position::FollowCursor,
    )
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
    list_card(
        String::from("Optional dependencies"),
        String::from("No suggestions"),
        top.iter().map(optdep_row).collect(),
        pad,
    )
}

fn recent_row(pkg: &RecentPkg) -> Element<'static> {
    let label = Row::new()
        .align_y(Alignment::Center)
        .spacing(8.0)
        .width(Length::Shrink)
        .push(ellipsized_body(pkg.name.clone(), Length::Shrink))
        .push(muted(text::caption(format!("({})", pkg.version))));
    searchable_row(pkg.name.clone(), label.into(), pkg.age.clone())
}

fn recent_card(recent: &[RecentPkg], pad: f32) -> Element<'static> {
    list_card(
        String::from("Recently updated"),
        String::from("No recent activity"),
        recent.iter().map(recent_row).collect(),
        pad,
    )
}

fn skeleton_card(pad: f32) -> Element<'static> {
    let mut rows = Column::new();
    for _ in 0..CARD_ROWS {
        rows = rows
            .push(container(hidden(text::body(String::from("package-name")))).padding([4.0, 0.0]));
    }
    container(
        Column::new()
            .spacing(8.0)
            .push(hidden(text::heading(String::from("Placeholder"))))
            .push(rows),
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
        None => skeleton_band(),
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
            .push(skeleton_card(pad))
            .push(skeleton_card(pad)),
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
