use cosmic::iced::{Alignment, Background, Border, Length};
use cosmic::widget::{Column, Row, button, container, space, text};

use super::theme::{accent_color, muted_color};

pub struct OnboardingPane {
    pub(crate) step: Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Safety,
    Setup,
}

#[derive(Clone, Debug)]
pub enum OnboardingMessage {
    Next,
    Back,
    Skip,
    Finish,
}

impl OnboardingPane {
    pub(crate) fn update(&mut self, message: OnboardingMessage) {
        match message {
            OnboardingMessage::Next => {
                self.step = match self.step {
                    Step::Welcome => Step::Safety,
                    Step::Safety => Step::Setup,
                    Step::Setup => Step::Setup,
                };
            }
            OnboardingMessage::Back => {
                self.step = match self.step {
                    Step::Welcome => Step::Welcome,
                    Step::Safety => Step::Welcome,
                    Step::Setup => Step::Safety,
                };
            }
            OnboardingMessage::Skip | OnboardingMessage::Finish => {}
        }
    }

    pub fn view(&self) -> crate::Element<'_> {
        let (space_xxs, space_m, space_l) = {
            let spacing = cosmic::theme::spacing();
            (spacing.space_xxs, spacing.space_m, spacing.space_l)
        };
        let column = Column::new()
            .push(self.dots())
            .push(space::vertical().height(space_m))
            .push(text::title2(self.step.title()).width(Length::Fill).center())
            .push(space::vertical().height(space_l))
            .push(
                container(self.content())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .push(space::vertical().height(space_m))
            .push(self.footer(space_xxs))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center);
        container(
            container(column)
                .width(Length::Fill)
                .max_width(640.0)
                .height(Length::Fill)
                .max_height(448.0)
                .padding(space_l),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(theme.cosmic().palette.neutral_3.into())),
            ..Default::default()
        })
        .into()
    }

    fn content(&self) -> crate::Element<'_> {
        text::body(self.step.body())
            .width(Length::Fill)
            .center()
            .into()
    }

    fn dots(&self) -> crate::Element<'static> {
        container(
            Row::new()
                .spacing(8.0)
                .push(dot(self.step == Step::Welcome))
                .push(dot(self.step == Step::Safety))
                .push(dot(self.step == Step::Setup)),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
    }

    fn footer(&self, space_xxs: u16) -> crate::Element<'static> {
        let mut footer = Row::new()
            .width(Length::Fill)
            .spacing(space_xxs)
            .align_y(Alignment::Center)
            .push(
                button::link("Skip").on_press(crate::Message::Onboarding(OnboardingMessage::Skip)),
            )
            .push(space::horizontal().width(Length::Fill));
        if self.step != Step::Welcome {
            footer = footer.push(
                button::standard("Back")
                    .on_press(crate::Message::Onboarding(OnboardingMessage::Back)),
            );
        }
        let primary = match self.step {
            Step::Setup => button::suggested("Finish")
                .on_press(crate::Message::Onboarding(OnboardingMessage::Finish)),
            Step::Welcome | Step::Safety => button::suggested("Next")
                .on_press(crate::Message::Onboarding(OnboardingMessage::Next)),
        };
        footer = footer.push(primary);
        footer.into()
    }
}

impl Step {
    fn title(&self) -> &'static str {
        match self {
            Step::Welcome => "Welcome to pakajo",
            Step::Safety => "A quick note on safety",
            Step::Setup => "Settings",
        }
    }

    fn body(&self) -> &'static str {
        match self {
            Step::Welcome => "A simple, modern package manager for Arch Linux.",
            Step::Safety => "aur scawy [fear]",
            Step::Setup => "some setting here or smth",
        }
    }
}

fn dot(active: bool) -> crate::Element<'static> {
    container(space::horizontal())
        .width(8)
        .height(8)
        .style(move |theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(if active {
                accent_color(theme)
            } else {
                muted_color(theme)
            })),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
