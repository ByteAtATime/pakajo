use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Background, Border, Color, Length, Shadow};
use cosmic::widget::{Column, Row, button, container, space, text};
use pakajo::transaction_state::{RepoStage, RepoState};

use super::TransactionMessage;
use super::shared::{accent_color, download_view, muted, muted_color, on_color, tinted};
use super::state::{StageState, TransactionModel};
use crate::Element;
use crate::components::divider::divider;

pub(super) fn action_footer() -> Element<'static> {
    let header_divider = divider();
    let close =
        button::standard("Close").on_press(crate::Message::Transaction(TransactionMessage::Close));
    Column::new()
        .spacing(12)
        .push(header_divider)
        .push(Row::new().push(space::horizontal()).push(close))
        .into()
}

pub(super) fn stage_row(model: &TransactionModel, i: usize, stage: RepoStage) -> Element<'_> {
    let state = model.stage_state(i);
    let label = stage_label(stage);
    let gutter = stage_glyph(state);
    let header = header_row(state, label);

    let content: Element<'_> = match state {
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
        .align_y(Vertical::Top)
        .push(gutter)
        .push(container(content).width(Length::Fill));

    container(body)
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(move |theme: &cosmic::Theme| stage_panel_style(theme, state))
        .into()
}

const GLYPH_GUTTER_WIDTH: f32 = 18.0;

fn stage_glyph(state: StageState) -> Element<'static> {
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

fn header_row(state: StageState, label: &'static str) -> Element<'static> {
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
                border: Border {
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
                border: Border {
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
            border: Border {
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
            border: Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color { a: 0.4, ..divider },
            },
            ..Default::default()
        },
    }
}

fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

fn active_view(state: &RepoState, stage: RepoStage) -> Element<'_> {
    match stage {
        RepoStage::Download => download_view(state),
        _ => text(format!("running phase {}", stage_label(stage))).into(),
    }
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
