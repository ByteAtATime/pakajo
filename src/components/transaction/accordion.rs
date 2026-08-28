use cosmic::iced::{Background, Border, Color, Length, Shadow, widget::progress_bar};
use cosmic::widget::{Column, Row, button, container, space, text};
use pakajo::transaction_state::{DownloadFile, RepoStage, RepoState};
use pakajo::utils::{format_bytes, format_eta, humanize_size};

use super::TransactionMessage;
use super::state::{StageState, TransactionModel};

pub(super) fn action_footer() -> cosmic::Element<'static, crate::Message> {
    let divider = container(space::horizontal())
        .width(Length::Fill)
        .height(1.0)
        .style(|t: &cosmic::Theme| container::Style {
            background: Some(Background::Color(divider_color(t))),
            ..Default::default()
        });
    let close = button::custom(text("Close"))
        .on_press(crate::Message::Transaction(TransactionMessage::Close));
    Column::new()
        .spacing(12)
        .push(divider)
        .push(Row::new().push(space::horizontal()).push(close))
        .into()
}

pub(super) fn stage_row(
    model: &TransactionModel,
    i: usize,
    stage: RepoStage,
) -> cosmic::Element<'_, crate::Message> {
    let state = model.stage_state(i);
    let label = stage_label(stage);
    let gutter = stage_glyph(state);
    let header = header_row(state, label);

    let content: cosmic::Element<'_, crate::Message> = match state {
        StageState::Pending => header,
        StageState::Active => Column::new()
            .spacing(6)
            .push(header)
            .push(muted(active_view(&model.repo_state, stage)))
            .into(),
        StageState::Failed => Column::new()
            .spacing(6)
            .push(header)
            .push(muted(text("failed")))
            .into(),
        StageState::Done => {
            let toggle = button::custom(header)
                .padding([2.0, 0.0])
                .width(Length::Fill)
                .class(cosmic::theme::Button::Transparent)
                .on_press(crate::Message::Transaction(
                    TransactionMessage::ToggleStage(i),
                ));
            if model.expanded.contains(&i) {
                Column::new()
                    .spacing(6)
                    .push(toggle)
                    .push(muted(text("Completed")))
                    .into()
            } else {
                toggle.into()
            }
        }
    };

    let body = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Top)
        .push(gutter)
        .push(container(content).width(Length::Fill));

    container(body)
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(move |theme: &cosmic::Theme| stage_panel_style(theme, state))
        .into()
}

const GLYPH_GUTTER_WIDTH: f32 = 18.0;
const STREAM_ROW_HEIGHT: f32 = 36.0;

fn stage_glyph(state: StageState) -> cosmic::Element<'static, crate::Message> {
    let glyph_text: &'static str = match state {
        StageState::Done => "✓",
        StageState::Active => "●",
        StageState::Pending => "○",
        StageState::Failed => "✗",
    };
    let glyph_color_fn: fn(&cosmic::Theme) -> Color = match state {
        StageState::Done => accent_color,
        StageState::Active => accent_color,
        StageState::Pending => muted_color,
        StageState::Failed => destructive_color,
    };
    container(tinted(text(glyph_text), glyph_color_fn))
        .width(Length::Fixed(GLYPH_GUTTER_WIDTH))
        .into()
}

fn header_row(state: StageState, label: &'static str) -> cosmic::Element<'static, crate::Message> {
    let label_widget = match state {
        StageState::Active => text(label).font(cosmic::font::bold()).size(18.0),
        StageState::Done => text(label).font(cosmic::font::semibold()),
        _ => text(label),
    };
    let label_color_fn: fn(&cosmic::Theme) -> Color = match state {
        StageState::Done => on_color,
        StageState::Active => accent_color,
        StageState::Pending => muted_color,
        StageState::Failed => on_color,
    };

    Row::new()
        .width(Length::Fill)
        .spacing(8)
        .push(tinted(label_widget, label_color_fn))
        .push(space::horizontal())
        .into()
}

fn muted<'a>(
    content: impl Into<cosmic::Element<'a, crate::Message>>,
) -> cosmic::Element<'a, crate::Message> {
    container(content)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(muted_color(t)),
            ..Default::default()
        })
        .into()
}

fn tinted<'a>(
    content: impl Into<cosmic::Element<'a, crate::Message>>,
    color_fn: fn(&cosmic::Theme) -> Color,
) -> cosmic::Element<'a, crate::Message> {
    container(content.into())
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(color_fn(theme)),
            ..Default::default()
        })
        .into()
}

fn stage_panel_style(theme: &cosmic::Theme, state: StageState) -> container::Style {
    let cosmic = theme.cosmic();
    let surface_base = Color::from(cosmic.background(false).base);
    let surface_mid = Color::from(cosmic.background(false).small_widget);
    let surface_high = Color::from(cosmic.background(false).component.base);
    let divider = Color::from(cosmic.background(false).divider);
    let on = Color::from(cosmic.background(false).on);

    match state {
        StageState::Active => {
            let accent = Color::from(cosmic.accent.base);
            container::Style {
                text_color: Some(on),
                background: Some(Background::Color(surface_base)),
                border: cosmic::iced::Border {
                    radius: 8.0.into(),
                    width: 2.0,
                    color: accent,
                },
                shadow: Shadow {
                    color: Color { a: 0.10, ..accent },
                    offset: Default::default(),
                    blur_radius: 15.0,
                },
                ..Default::default()
            }
        }
        StageState::Failed => {
            let destructive = Color::from(cosmic.destructive.base);
            container::Style {
                text_color: Some(on),
                background: Some(Background::Color(surface_base)),
                border: cosmic::iced::Border {
                    radius: 8.0.into(),
                    width: 2.0,
                    color: destructive,
                },
                ..Default::default()
            }
        }
        StageState::Done => container::Style {
            text_color: Some(on),
            background: Some(Background::Color(surface_high)),
            border: cosmic::iced::Border {
                radius: 8.0.into(),
                width: 1.0,
                color: divider,
            },
            ..Default::default()
        },
        StageState::Pending => container::Style {
            text_color: Some(Color { a: 0.5, ..on }),
            background: Some(Background::Color(Color {
                a: 0.35,
                ..surface_mid
            })),
            border: cosmic::iced::Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color { a: 0.4, ..divider },
            },
            ..Default::default()
        },
    }
}

fn muted_color(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    Color { a: 0.5, ..on }
}

fn on_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().background(false).on)
}

fn accent_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().accent.base)
}

fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

fn divider_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().background(false).divider)
}

fn active_view(state: &RepoState, stage: RepoStage) -> cosmic::Element<'_, crate::Message> {
    match stage {
        RepoStage::Download => download_view(state),
        _ => text(format!("running phase {}", stage_label(stage))).into(),
    }
}

fn files_in_order(state: &RepoState) -> impl Iterator<Item = (&str, &DownloadFile)> {
    state
        .download_order
        .iter()
        .filter_map(|name| state.download_files.get(name).map(|f| (name.as_str(), f)))
}

fn active_card(filename: &str, file: &DownloadFile) -> cosmic::Element<'static, crate::Message> {
    let pct = if file.total > 0 {
        (file.downloaded as f64 / file.total as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let name = text(filename.to_string()).font(cosmic::font::mono());
    let name = cosmic::widget::container(name)
        .width(Length::Fill)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(on_color(t)),
            ..Default::default()
        });
    let pct_text = tinted(text(format!("{:.0}%", pct)), accent_color);
    let top = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .push(name)
        .push(space::horizontal())
        .push(pct_text);
    let bar = cosmic::iced::widget::progress_bar(0.0..=100.0, pct as f32)
        .length(Length::Fill)
        .girth(6.0);
    let eta_str = if file.total > file.downloaded && file.rate > 0.0 {
        let remaining = (file.total - file.downloaded) as f64 / file.rate;
        format_eta(remaining.ceil() as u64)
    } else if file.total > 0 && file.downloaded >= file.total {
        format_eta(0)
    } else {
        "--:--".to_string()
    };
    let bottom = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .push(muted(text(format!(
            "{} / {}",
            format_bytes(file.downloaded),
            format_bytes(file.total)
        ))))
        .push(space::horizontal())
        .push(muted(text(format!("ETA: {}", eta_str))));

    Column::new()
        .spacing(6)
        .push(top)
        .push(bar)
        .push(bottom)
        .into()
}

fn completed_row(filename: &str, file: &DownloadFile) -> cosmic::Element<'static, crate::Message> {
    let name_widget = container(text(filename.to_string()).font(cosmic::font::mono()))
        .width(Length::Fill)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(on_color(t)),
            ..Default::default()
        });
    Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .spacing(8)
        .push(
            crate::components::icons::circle_check()
                .width(14.0)
                .height(14.0),
        )
        .push(name_widget)
        .push(space::horizontal())
        .push(muted(text(format_bytes(file.total))))
        .into()
}

fn stream_row(filename: &str, file: &DownloadFile) -> cosmic::Element<'static, crate::Message> {
    let pct = if file.total > 0 {
        (file.downloaded as f64 / file.total as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let background = progress_bar(0.0..=100.0, pct as f32)
        .length(Length::Fill)
        .girth(STREAM_ROW_HEIGHT)
        .class(cosmic::theme::ProgressBar::custom(
            |theme: &cosmic::Theme| {
                let accent = accent_color(theme);
                let on = on_color(theme);
                cosmic::iced::widget::progress_bar::Style {
                    bar: Background::Color(Color { a: 0.18, ..accent }),
                    background: Background::Color(Color { a: 0.06, ..on }),
                    border: Border {
                        radius: 6.0.into(),
                        width: 0.0,
                        color: Color::TRANSPARENT,
                    },
                }
            },
        ));
    let (rate_val, rate_unit) = humanize_size(file.rate.max(0.0) as i64);
    let speed_str = format!("{:.2} {}/s", rate_val, rate_unit);
    let right = format!(
        "{} / {}  {}",
        format_bytes(file.downloaded),
        format_bytes(file.total),
        speed_str
    );
    let name = container(text(filename.to_string()).font(cosmic::font::mono()))
        .width(Length::Fill)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(on_color(t)),
            ..Default::default()
        });
    let foreground = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .padding([0.0, 12.0])
        .width(Length::Fill)
        .height(Length::Fill)
        .push(name)
        .push(space::horizontal())
        .push(
            container(text(right)).style(|t: &cosmic::Theme| container::Style {
                text_color: Some(on_color(t)),
                ..Default::default()
            }),
        );
    cosmic::iced::widget::Stack::new()
        .push(background)
        .push(foreground)
        .width(Length::Fill)
        .into()
}

fn rich_view(state: &RepoState) -> cosmic::Element<'_, crate::Message> {
    let mut col = Column::new().spacing(8);
    let completed: Vec<(&str, &DownloadFile)> =
        files_in_order(state).filter(|(_, f)| f.completed).collect();
    let active: Vec<(&str, &DownloadFile)> = files_in_order(state)
        .filter(|(_, f)| !f.completed)
        .collect();
    if !completed.is_empty() {
        col = col.push(muted(text("Completed")));
        for (filename, file) in completed {
            col = col.push(completed_row(filename, file));
        }
    }
    for (filename, file) in active {
        col = col.push(active_card(filename, file));
    }
    col.into()
}

fn compact_view(state: &RepoState) -> cosmic::Element<'_, crate::Message> {
    let total = state.download_bytes_total.max(0);
    let done = state.download_bytes_done.max(0);
    let pct = if total > 0 {
        (done as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let top_row = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .push(muted(text("Overall Progress")))
        .push(space::horizontal())
        .push(tinted(text(format!("{:.0}%", pct)), accent_color));

    let bar = progress_bar(0.0..=100.0, pct as f32)
        .length(Length::Fill)
        .girth(6.0);

    let eta_str = if total > done && state.download_rate > 0.0 {
        let remaining = (total - done) as f64 / state.download_rate;
        format_eta(remaining.ceil() as u64)
    } else if total > 0 && done >= total {
        format_eta(0)
    } else {
        "--:--".to_string()
    };

    let bottom_row = Row::new()
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .push(muted(text(format!(
            "{} / {} packages",
            state.download_done, state.download_total
        ))))
        .push(space::horizontal())
        .push(muted(text(format!(
            "{} / {}  ETA: {}",
            format_bytes(done),
            format_bytes(total),
            eta_str
        ))));

    let mut col = Column::new().spacing(8);
    col = col.push(top_row);
    col = col.push(bar);
    col = col.push(bottom_row);

    let active: Vec<(&str, &DownloadFile)> = files_in_order(state)
        .filter(|(_, f)| !f.completed)
        .collect();
    col = col.push(muted(text(format!("Downloading ({})", active.len()))));
    for (filename, file) in active {
        col = col.push(stream_row(filename, file));
    }
    col = col.push(muted(text(format!(
        "{} packages remaining in queue",
        state.queued()
    ))));
    col.into()
}

fn download_view(state: &RepoState) -> cosmic::Element<'_, crate::Message> {
    if state.download_total < 4 {
        return rich_view(state);
    }
    compact_view(state)
}

fn stage_label(stage: RepoStage) -> &'static str {
    match stage {
        RepoStage::Resolve => "Resolve",
        RepoStage::Validate => "Validate",
        RepoStage::Download => "Download",
        RepoStage::Install => "Install",
        RepoStage::Finalize => "Finalize",
    }
}
