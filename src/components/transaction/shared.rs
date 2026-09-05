use std::borrow::Cow;

use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::{Stack, progress_bar};
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, space, text};
use pakajo::transaction_state::{DownloadFile, DownloadState};
use pakajo::utils::{format_bytes, format_eta, humanize_size};

use crate::Element;
use crate::components::icons::circle_check;

const STREAM_ROW_HEIGHT: f32 = 36.0;

pub(super) fn mono_text(name: &str) -> Element<'static> {
    tinted(text(name.to_string()).font(cosmic::font::mono()), on_color)
}

fn file_name(filename: &str) -> Element<'static> {
    container(mono_text(filename)).width(Length::Fill).into()
}

pub(super) fn thin_bar(value: f32) -> Element<'static> {
    progress_bar(0.0..=100.0, value)
        .length(Length::Fill)
        .girth(6.0)
        .into()
}

pub(super) fn muted<'a>(content: impl Into<Element<'a>>) -> Element<'a> {
    container(content)
        .style(|t: &cosmic::Theme| container::Style {
            text_color: Some(muted_color(t)),
            ..Default::default()
        })
        .into()
}

pub(super) fn tinted<'a>(
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

pub(super) fn muted_color(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    Color { a: 0.5, ..on }
}

pub(super) fn on_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().background(false).on)
}

pub(super) fn accent_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().accent.base)
}

pub(super) fn success_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().success.base)
}

pub(super) fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

pub(super) fn warning_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().warning.base)
}

pub(super) fn pill(
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

pub(super) fn version_change(old: Option<&str>, new: Option<&str>) -> String {
    match (old, new) {
        (Some(old), Some(new)) => format!("{old} → {new}"),
        (None, Some(new)) => new.to_string(),
        (Some(old), None) => old.to_string(),
        (None, None) => String::new(),
    }
}

pub(super) fn percent(done: i64, total: i64) -> f64 {
    if total <= 0 {
        return 0.0;
    }
    (done as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
}

pub(super) fn eta(done: i64, total: i64, rate: f64) -> String {
    if total > done && rate > 0.0 {
        let remaining = (total - done) as f64 / rate;
        return format_eta(remaining.ceil() as u64);
    }
    if total > 0 && done >= total {
        return format_eta(0);
    }
    "--:--".to_string()
}

fn files_in_order(state: &DownloadState) -> impl Iterator<Item = (&str, &DownloadFile)> {
    state
        .order
        .iter()
        .filter_map(|name| state.files.get(name).map(|f| (name.as_str(), f)))
}

fn active_card(filename: &str, file: &DownloadFile) -> Element<'static> {
    let pct = percent(file.downloaded, file.total);
    let pct_text = tinted(text(format!("{:.0}%", pct)), accent_color);
    let top = Row::new()
        .align_y(Vertical::Center)
        .push(file_name(filename))
        .push(space::horizontal())
        .push(pct_text);
    let bar = thin_bar(pct as f32);
    let eta_str = eta(file.downloaded, file.total, file.rate);
    let bottom = Row::new()
        .align_y(Vertical::Center)
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

fn completed_row(filename: &str, file: &DownloadFile) -> Element<'static> {
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(cosmic::widget::icon(circle_check()).size(14))
        .push(file_name(filename))
        .push(space::horizontal())
        .push(muted(text(format_bytes(file.total))))
        .into()
}

fn stream_row(filename: &str, file: &DownloadFile) -> Element<'static> {
    let pct = percent(file.downloaded, file.total);
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
                        radius: theme.cosmic().corner_radii.radius_s.into(),
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
    let name = file_name(filename);
    let foreground = Row::new()
        .align_y(Vertical::Center)
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
    Stack::new()
        .push(background)
        .push(foreground)
        .width(Length::Fill)
        .into()
}

fn rich_view(state: &DownloadState) -> Element<'_> {
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

fn compact_view(state: &DownloadState) -> Element<'_> {
    let total = state.bytes_total.max(0);
    let done = state.bytes_done.max(0);
    let pct = percent(done, total);

    let top_row = Row::new()
        .align_y(Vertical::Center)
        .push(muted(text("Overall Progress")))
        .push(space::horizontal())
        .push(tinted(text(format!("{:.0}%", pct)), accent_color));

    let bar = thin_bar(pct as f32);

    let eta_str = eta(done, total, state.rate);

    let bottom_row = Row::new()
        .align_y(Vertical::Center)
        .push(muted(text(format!(
            "{} / {} packages",
            state.done, state.total
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

pub(super) fn counter_suffix(done: usize, total: usize, unit: &str) -> Element<'static> {
    muted(text(format!("{done} / {total} {unit}")))
}

pub(crate) struct ResolvedEntry<'a> {
    pub qualified: Cow<'a, str>,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub net_size: Option<i64>,
}

pub(crate) fn resolve_package_row(entry: &ResolvedEntry<'_>) -> Element<'static> {
    let left = mono_text(&entry.qualified);
    let mut right = Row::new().align_y(Vertical::Center).spacing(8).push(muted(
        text(version_change(entry.old_version, entry.new_version)).font(cosmic::font::mono()),
    ));
    if let Some(net) = entry.net_size {
        right = right.push(text(format_signed_bytes(net)));
    }
    Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(left)
        .push(space::horizontal())
        .push(right)
        .into()
}

pub(crate) fn resolve_single_suffix(entry: &ResolvedEntry<'_>) -> Element<'static> {
    let name_version = match entry.new_version {
        Some(version) if !version.is_empty() => {
            format!("{} {version}", entry.qualified)
        }
        _ => entry.qualified.to_string(),
    };
    let mut row = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(muted(text(name_version).font(cosmic::font::mono())));
    if let Some(net) = entry.net_size {
        let size_color: fn(&cosmic::Theme) -> Color = if net >= 0 {
            accent_color
        } else {
            success_color
        };
        row = row.push(pill(format_signed_bytes(net), size_color));
    }
    row.into()
}

pub(crate) fn resolve_empty_view() -> Element<'static> {
    muted(text("Nothing to do"))
}

pub(crate) fn format_signed_bytes(value: i64) -> String {
    if value < 0 {
        format!("-{}", format_bytes(value.abs()))
    } else {
        format!("+{}", format_bytes(value))
    }
}

pub(super) fn download_view(state: &DownloadState) -> Element<'_> {
    if state.total < 4 {
        return rich_view(state);
    }
    compact_view(state)
}

#[cfg(test)]
mod tests {
    use super::{eta, percent};

    #[test]
    fn percent_zero_done() {
        assert_eq!(percent(0, 100), 0.0);
    }

    #[test]
    fn percent_half_done() {
        assert_eq!(percent(50, 100), 50.0);
    }

    #[test]
    fn percent_full() {
        assert_eq!(percent(100, 100), 100.0);
    }

    #[test]
    fn percent_clamps_above_total() {
        assert_eq!(percent(150, 100), 100.0);
    }

    #[test]
    fn percent_zero_total() {
        assert_eq!(percent(0, 0), 0.0);
    }

    #[test]
    fn eta_zero_rate() {
        assert_eq!(eta(10, 100, 0.0), "--:--");
    }

    #[test]
    fn eta_completed() {
        assert_eq!(eta(100, 100, 10.0), "00:00");
    }

    #[test]
    fn eta_mid_download() {
        assert_eq!(eta(0, 100, 10.0), "00:10");
    }
}
