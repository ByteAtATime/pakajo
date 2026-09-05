use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, container, scrollable, text};
use pakajo::transaction_state::FinalizeState;

use crate::Element;

use super::accordion::Section;
use super::state::StageState;

const LOG_HEIGHT: f32 = 200.0;

pub(super) fn finalize_section(finalize: &FinalizeState, state: StageState) -> Section<'_> {
    Section {
        label: "Finalize",
        state,
        content: match state {
            StageState::Active | StageState::Done if !finalize.lines.is_empty() => {
                Some(finalize_log(&finalize.lines))
            }
            _ => None,
        },
        header_suffix: None,
    }
}

pub(super) fn finalize_log(lines: &[String]) -> Element<'_> {
    let mut col = Column::new().spacing(2);
    for line in lines {
        col = col.push(text(line.clone()).font(cosmic::font::mono()));
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
                    radius: 6.0.into(),
                    width: 1.0,
                    color: Color::from(cosmic.background(false).divider),
                },
                ..Default::default()
            }
        })
        .into()
}
