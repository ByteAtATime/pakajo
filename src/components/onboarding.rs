use cosmic::iced::widget::{rich_text, span};
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{Column, Row, Space, button, checkbox, container, mouse_area, svg, text};

use crate::components::icons;
use crate::components::theme::{accent_color, card_style, muted, muted_color, warning_color};

const BODY_LINE_HEIGHT: f32 = 1.6;

pub struct OnboardingPane {
    pub(crate) step: Step,
    pub(crate) keep_cache: bool,
    pub(crate) link_hovered: bool,
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
    ToggleKeepCache(bool),
    HoverLink(bool),
    Skip,
    Finish,
}

impl OnboardingPane {
    pub fn new(keep_cache: bool) -> Self {
        Self {
            step: Step::Welcome,
            keep_cache,
            link_hovered: false,
        }
    }

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
            OnboardingMessage::ToggleKeepCache(keep_cache) => self.keep_cache = keep_cache,
            OnboardingMessage::HoverLink(hovered) => self.link_hovered = hovered,
            OnboardingMessage::Skip | OnboardingMessage::Finish => {}
        }
    }

    pub fn view(&self) -> crate::Element<'_> {
        let spacing = cosmic::theme::spacing();
        let (space_s, space_m, space_l) = (spacing.space_s, spacing.space_m, spacing.space_l);

        let mut header = Column::new()
            .spacing(space_s)
            .align_x(Alignment::Center)
            .push(self.dots());

        if self.step != Step::Welcome {
            header = header
                .push(text::title2(self.step.title()).center())
                .push_maybe(
                    (!self.step.subtitle().is_empty()).then(|| muted(self.step.subtitle())),
                );
        }

        let body = container(self.content())
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center);

        let column = Column::new()
            .spacing(space_m)
            .push(header)
            .push(body)
            .push(self.footer())
            .width(Length::Fill)
            .height(Length::Fill);

        container(
            container(column)
                .width(Length::Fill)
                .max_width(680.0)
                .height(Length::Fill)
                .max_height(480.0)
                .padding(space_l)
                .style(card_style),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.65))),
            ..Default::default()
        })
        .into()
    }

    fn content(&self) -> crate::Element<'_> {
        match self.step {
            Step::Welcome => self.view_welcome(),
            Step::Safety => self.view_safety(),
            Step::Setup => self.view_setup(),
        }
    }

    fn view_welcome(&self) -> crate::Element<'_> {
        let space_m = cosmic::theme::spacing().space_m;

        Column::new()
            .spacing(space_m)
            .align_x(Alignment::Center)
            .push(text::title2(self.step.title()).center())
            .push(muted(self.step.subtitle()))
            .into()
    }

    fn view_safety(&self) -> crate::Element<'_> {
        let spacing = cosmic::theme::spacing();
        let theme = cosmic::theme::active();
        let danger = warning_color(&theme);
        let accent = Color::from(theme.cosmic().accent_text_color());

        Column::new()
            .spacing(spacing.space_s)
            .width(Length::Fill)
            .push(
                rich_text::<String, crate::Message, cosmic::Theme, cosmic::Renderer>([
                    span("The Arch User Repository is powered by the community, meaning "),
                    span("packages are not reviewed by Arch Linux")
                        .font(cosmic::font::semibold())
                        .color(danger),
                    span("."),
                ])
                .line_height(BODY_LINE_HEIGHT)
                .width(Length::Fill),
            )
            .push(
                rich_text::<String, crate::Message, cosmic::Theme, cosmic::Renderer>([span(
                    "While it gives you access to almost any software, malicious or outdated scripts can compromise your system.",
                )])
                .line_height(BODY_LINE_HEIGHT)
                .width(Length::Fill),
            )
            .push(
                rich_text::<String, crate::Message, cosmic::Theme, cosmic::Renderer>([
                    span("To stay safe, "),
                    span("always review what you're installing")
                        .font(cosmic::font::semibold())
                        .color(danger),
                    span(" and proceed with care!"),
                ])
                .line_height(BODY_LINE_HEIGHT)
                .width(Length::Fill),
            )
            .push(
                mouse_area(
                    Row::new()
                        .spacing(2)
                        .align_y(Alignment::Center)
                        .push(
                            rich_text::<String, crate::Message, cosmic::Theme, cosmic::Renderer>(
                                [span("Learn more")
                                    .color(accent)
                                    .underline(self.link_hovered)],
                            )
                            .line_height(BODY_LINE_HEIGHT),
                        )
                        .push(
                            svg(icons::arrow_right_svg())
                                .width(Length::Fixed(14.0))
                                .height(Length::Fixed(14.0))
                                .class(cosmic::theme::Svg::custom(move |_theme| svg::Style {
                                    color: Some(accent),
                                })),
                        ),
                )
                .on_enter(crate::Message::Onboarding(OnboardingMessage::HoverLink(true)))
                .on_exit(crate::Message::Onboarding(OnboardingMessage::HoverLink(false)))
                .interaction(cosmic::iced::mouse::Interaction::Pointer)
                .on_press(crate::Message::OpenUrl("https://wiki.archlinux.org/title/Arch_User_Repository".to_string())),
            )
            .into()
    }

    fn view_setup(&self) -> crate::Element<'_> {
        let spacing = cosmic::theme::spacing();
        let (space_xxs, space_s, space_m) = (spacing.space_xxs, spacing.space_s, spacing.space_m);

        // TODO: dim this on card hover or something
        let cache_toggle = checkbox(self.keep_cache)
            .label("Keep build files for faster updates")
            .on_toggle(|val| crate::Message::Onboarding(OnboardingMessage::ToggleKeepCache(val)));

        Column::new()
            .spacing(space_m)
            .push(
                container(
                    Column::new()
                        .spacing(space_xxs)
                        .push(cache_toggle)
                        .push(muted(
                            "Caches downloaded sources to speed up future AUR builds; turn off if you prefer to conserve disk space.",
                        )),
                )
                .padding(space_s)
                .width(Length::Fill)
                .style(card_style),
            )
            .into()
    }

    fn dots(&self) -> crate::Element<'static> {
        container(
            Row::new()
                .spacing(cosmic::theme::spacing().space_xs)
                .push(dot(self.step == Step::Welcome))
                .push(dot(self.step == Step::Safety))
                .push(dot(self.step == Step::Setup)),
        )
        .center_x(Length::Fill)
        .into()
    }

    fn footer(&self) -> crate::Element<'static> {
        let mut footer = Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .spacing(8)
            .push(
                button::link("Skip").on_press(crate::Message::Onboarding(OnboardingMessage::Skip)),
            )
            .push(Space::new().width(Length::Fill));

        if self.step != Step::Welcome {
            footer = footer.push(
                button::standard("Back")
                    .on_press(crate::Message::Onboarding(OnboardingMessage::Back)),
            );
        }

        let primary = match self.step {
            Step::Setup => button::suggested("Start using Pakajo!")
                .on_press(crate::Message::Onboarding(OnboardingMessage::Finish)),
            Step::Welcome | Step::Safety => button::suggested("Next")
                .on_press(crate::Message::Onboarding(OnboardingMessage::Next)),
        };

        footer.push(primary).into()
    }
}

impl Step {
    fn title(&self) -> &'static str {
        match self {
            Step::Welcome => "Welcome to Pakajo!",
            Step::Safety => "Staying safe on the AUR",
            Step::Setup => "Preferences",
        }
    }

    fn subtitle(&self) -> &'static str {
        match self {
            Step::Welcome => "An elegant, modern package manager for Arch Linux",
            Step::Safety => "",
            Step::Setup => "Choose how Pakajo handles builds and reviews",
        }
    }
}

fn dot(active: bool) -> crate::Element<'static> {
    container(Space::new().width(8).height(8))
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
