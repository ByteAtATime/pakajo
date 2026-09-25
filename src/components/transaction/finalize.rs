use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, container, scrollable, text};
use pakajo::events::LogLevel;
use pakajo::progress::FinalizeState;

use crate::Element;

use super::shared::{destructive_color, muted, tinted, warning_color};
use super::state::StageState;
use super::stepper::Section;

const LOG_HEIGHT: f32 = 200.0;

pub(super) fn finalize_section(finalize: &FinalizeState, state: StageState) -> Section<'_> {
    let mut section = Section::new("Finalize", state);
    if !finalize.is_empty() {
        section.content = Some(finalize_log(finalize));
    }
    let alerts = finalize
        .alerts
        .iter()
        .filter(|(level, _)| !matches!(level, LogLevel::Debug))
        .count();
    if alerts > 0 {
        section.summary = Some(muted(text(alert_summary(alerts))));
    }
    section
}

fn alert_summary(count: usize) -> String {
    if count == 1 {
        "1 alert".to_string()
    } else {
        format!("{count} alerts")
    }
}

pub(super) fn finalize_log(finalize: &FinalizeState) -> Element<'_> {
    let mut col = Column::new().spacing(2);
    for (level, message) in &finalize.alerts {
        let (prefix, color) = match level {
            LogLevel::Warning => ("warning:", warning_color as fn(&cosmic::Theme) -> Color),
            LogLevel::Error => ("error:", destructive_color as fn(&cosmic::Theme) -> Color),
            LogLevel::Debug => continue,
        };
        col = col.push(tinted(text::monotext(format!("{prefix} {message}")), color));
    }
    for line in &finalize.lines {
        col = col.push(text::monotext(line.clone()));
    }
    container(scrollable(col).height(Length::Fixed(LOG_HEIGHT)))
        .width(Length::Fill)
        .padding(8.0)
        .style(|theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            container::Style {
                background: Some(Background::Color(Color::from(
                    cosmic.background(false).component.base,
                ))),
                border: Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    width: 1.0,
                    color: Color::from(cosmic.background(false).divider),
                },
                ..Default::default()
            }
        })
        .into()
}
