use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, button, container, text};

pub struct OnboardingPane {
    pub(crate) step: Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
}

#[derive(Clone, Debug)]
pub enum OnboardingMessage {
    Finish,
}

impl OnboardingPane {
    pub fn view(&self) -> crate::Element<'_> {
        match self.step {
            Step::Welcome => self.welcome(),
        }
    }

    fn welcome(&self) -> crate::Element<'_> {
        let content = Column::new()
            .align_x(Alignment::Center)
            .spacing(12.0)
            .push(text::title1("Welcome to pakajo"))
            .push(text::body("something something package manager gui"))
            .push(
                button::suggested("Finish")
                    .on_press(crate::Message::Onboarding(OnboardingMessage::Finish)),
            );
        container(
            container(content)
                .width(Length::Shrink)
                .center_x(Length::Shrink),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    }
}
