use cosmic::app::Task;
use cosmic::iced::{Alignment, Color, Length};
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};

use pakajo::events::SummaryPackage;
use pakajo::upgrade::AurUpgradeCandidate;

use crate::Element;
use crate::components::icons;
use crate::components::sysupgrade::SysupgradeMessage;
use crate::components::theme;
use crate::components::theme::{muted_mono, muted_text as muted, themed_mono_text, themed_text};

#[derive(Clone, Debug)]
pub enum UpdatesMessage {
    RefreshUpdates,
    Fetched(Result<pakajo::updates::UpdatesFetch, String>),
}

#[derive(Clone, Debug, Default)]
pub enum UpdatesState {
    #[default]
    Idle,
    Loading,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshKind {
    Launch,
    ExternalChange,
    Interactive,
}

pub fn destructive_text_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().destructive_text_color().into()),
        ..Default::default()
    }
}

pub fn success_text_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().success_text_color().into()),
        ..Default::default()
    }
}

pub fn destructive<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a> {
    themed_text(content, destructive_text_style)
}

pub fn destructive_mono<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a> {
    themed_mono_text(content, destructive_text_style)
}

pub fn success_mono<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a> {
    themed_mono_text(content, success_text_style)
}

pub fn colored_version_delta(old: &str, new: &str) -> Element<'static> {
    let (common, old_suffix, new_suffix) = pakajo::utils::version_diff(old, new);
    Row::new()
        .align_y(Alignment::Center)
        .spacing(4)
        .push(
            Row::new()
                .push(muted_mono(common.clone()))
                .push(destructive_mono(old_suffix)),
        )
        .push(text(" \u{2192} "))
        .push(
            Row::new()
                .push(muted_mono(common))
                .push(success_mono(new_suffix)),
        )
        .into()
}

pub fn aur_version_delta<'a>(candidate: &'a AurUpgradeCandidate) -> Element<'a> {
    if candidate.remote_version == "latest-commit" {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(4)
            .push(success_mono(&candidate.local_version))
            .push(text(" \u{2192} "))
            .push(success_mono("latest-commit"))
            .into()
    } else {
        colored_version_delta(&candidate.local_version, &candidate.remote_version)
    }
}

const NAME_COLUMN_WIDTH: f32 = 320.0;
const LIST_MAX_WIDTH: f32 = 1000.0;
const SIZE_COLUMN_WIDTH: f32 = 180.0;

fn band_header(label: &str, count: usize) -> Element<'static> {
    let color = if label == "AUR" {
        theme::muted_color
    } else {
        theme::accent_color
    };
    container(theme::tinted(text(format!("{label} ({count})")), color))
        .width(Length::Fill)
        .padding([8.0, 16.0])
        .style(|theme: &cosmic::Theme| cosmic::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(Color::from(
                theme.cosmic().background(false).component.base,
            ))),
            ..Default::default()
        })
        .into()
}

fn stat_card(label: &str, value: String, sub: String) -> Element<'static> {
    container(
        Column::new()
            .align_x(Alignment::Center)
            .spacing(2.0)
            .push(theme::tinted(
                text::caption(label.to_owned()),
                theme::muted_color,
            ))
            .push(text::title3(value))
            .push(theme::tinted(text::caption(sub), theme::muted_color)),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .style(theme::card_style)
    .padding(12.0)
    .into()
}

fn stat_cards(pending: &pakajo::updates::PendingUpdates) -> Element<'static> {
    let gap = cosmic::theme::spacing().space_xs as f32;
    let repo_count = pending.repo.packages.len();
    let download: i64 = pending.repo.packages.iter().map(|p| p.download_size).sum();
    let disk: i64 = pending
        .repo
        .packages
        .iter()
        .map(|p| p.installed_size - p.old_installed_size)
        .sum();
    Row::new()
        .spacing(gap)
        .push(stat_card(
            "Packages",
            (repo_count + pending.aur.len()).to_string(),
            format!("{repo_count} repo \u{b7} {} AUR", pending.aur.len()),
        ))
        .push(stat_card(
            "Download size",
            pakajo::utils::format_bytes(download),
            String::from("excludes AUR"),
        ))
        .push(stat_card(
            "Disk impact",
            pakajo::utils::format_bytes(disk),
            String::from("excludes AUR"),
        ))
        .into()
}

mod spinner {
    use crate::Element;
    use cosmic::iced::advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, renderer, widget::Tree,
    };
    use cosmic::iced::{Event, Length, Radians, Rectangle, Rotation, Size, window};
    use cosmic::widget::Space;
    use std::time::Instant;

    pub struct Spinner {
        handle: cosmic::widget::svg::Handle,
        start: Instant,
        svg: cosmic::widget::Svg<'static, cosmic::Theme>,
    }

    impl Spinner {
        pub fn new(handle: cosmic::widget::svg::Handle, start: Instant) -> Self {
            Self {
                svg: Self::svg(&handle, start, Instant::now()),
                handle,
                start,
            }
        }

        fn svg(
            handle: &cosmic::widget::svg::Handle,
            start: Instant,
            now: Instant,
        ) -> cosmic::widget::Svg<'static, cosmic::Theme> {
            let angle = -(now.saturating_duration_since(start).as_secs_f32() * 180.0);
            cosmic::widget::svg(handle.clone())
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .symbolic(true)
                .rotation(Rotation::Floating(Radians(angle)))
        }
    }

    impl<Message: 'static> Widget<Message, cosmic::Theme, cosmic::Renderer> for Spinner {
        fn size(&self) -> Size<Length> {
            Size::new(Length::Fixed(16.0), Length::Fixed(16.0))
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &cosmic::Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            <cosmic::widget::Svg<'static, cosmic::Theme> as Widget<
                Message,
                cosmic::Theme,
                cosmic::Renderer,
            >>::layout(&mut self.svg, tree, renderer, limits)
        }

        fn update(
            &mut self,
            _tree: &mut Tree,
            event: &Event,
            _layout: Layout<'_>,
            _cursor: mouse::Cursor,
            _renderer: &cosmic::Renderer,
            _clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
            _viewport: &Rectangle,
        ) {
            if let Event::Window(window::Event::RedrawRequested(now)) = event {
                self.svg = Self::svg(&self.handle, self.start, *now);
                shell.request_redraw();
            }
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut cosmic::Renderer,
            theme: &cosmic::Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            <cosmic::widget::Svg<'static, cosmic::Theme> as Widget<
                Message,
                cosmic::Theme,
                cosmic::Renderer,
            >>::draw(
                &self.svg, tree, renderer, theme, style, layout, cursor, viewport,
            );
        }
    }

    pub fn spinner(start: Option<Instant>) -> Element<'static> {
        let start = start.unwrap_or_else(Instant::now);
        let icon = cosmic::widget::icon(crate::components::icons::refresh_cw());
        let Some(handle) = icon.into_svg_handle() else {
            return Element::new(Space::new().width(Length::Fixed(16.0)));
        };
        Element::new(Spinner::new(handle, start))
    }
}

fn right_cell(content: Element<'static>, width: f32) -> Element<'static> {
    container(content)
        .width(Length::Fixed(width))
        .align_x(Alignment::End)
        .into()
}

fn size_cell(pkg: &SummaryPackage) -> Element<'static> {
    let delta = pkg.installed_size - pkg.old_installed_size;
    let cell = if delta > 0 {
        destructive_mono(format!("+{}", pakajo::utils::format_bytes(delta)))
    } else if delta < 0 {
        success_mono(format!("-{}", pakajo::utils::format_bytes(-delta)))
    } else if pkg.old_installed_size == 0 {
        destructive_mono(format!(
            "+{}",
            pakajo::utils::format_bytes(pkg.download_size)
        ))
    } else {
        muted_mono("0 B")
    };
    right_cell(cell, SIZE_COLUMN_WIDTH)
}

fn package_cell(repo: Option<&str>, name: String) -> Element<'static> {
    let prefix = repo.unwrap_or("other").to_lowercase();
    Row::new()
        .align_y(Alignment::Center)
        .spacing(0)
        .push(muted_mono(format!("{prefix}/")))
        .push(crate::components::row_title(name))
        .into()
}

pub fn table_row(pkg: &SummaryPackage, index: usize) -> Element<'_> {
    let old = pkg.old_version.clone().unwrap_or_default();
    let repo = pkg.repository.as_deref();
    padded_row(
        index,
        Row::new()
            .align_y(Alignment::Center)
            .spacing(12)
            .push(
                container(package_cell(repo, pkg.name.clone()))
                    .width(Length::Fixed(NAME_COLUMN_WIDTH)),
            )
            .push(container(colored_version_delta(&old, &pkg.new_version)).width(Length::Fill))
            .push(size_cell(pkg)),
    )
}

fn padded_row<'a>(
    index: usize,
    row: cosmic::widget::Row<'a, crate::Message, cosmic::Theme>,
) -> Element<'a> {
    container(row)
        .width(Length::Fill)
        .padding([6.0, 16.0])
        .style(move |theme: &cosmic::Theme| {
            let background = if index % 2 == 1 {
                cosmic::iced::Background::Color(cosmic::iced::Color {
                    a: 0.04,
                    ..Color::from(theme.cosmic().background(false).on)
                })
            } else {
                cosmic::iced::Background::Color(cosmic::iced::Color::TRANSPARENT)
            };
            cosmic::widget::container::Style {
                background: Some(background),
                ..Default::default()
            }
        })
        .into()
}

pub fn aur_table_row(c: &AurUpgradeCandidate, index: usize) -> Element<'_> {
    padded_row(
        index,
        Row::new()
            .align_y(Alignment::Center)
            .spacing(12)
            .push(
                container(package_cell(Some("aur"), c.name.clone()))
                    .width(Length::Fixed(NAME_COLUMN_WIDTH)),
            )
            .push(container(aur_version_delta(c)).width(Length::Fill))
            .push(Space::new().width(Length::Fixed(SIZE_COLUMN_WIDTH))),
    )
}

#[derive(Default)]
pub struct UpdatesPane {
    pub(crate) state: UpdatesState,
    pub(crate) pending: pakajo::updates::PendingUpdates,
    pub(crate) count: u32,
    pub(crate) aur_error: Option<String>,
    pub(crate) last_cache: Option<pakajo::updates::UpdatesCache>,
    pub(crate) refreshing: bool,
    pub(crate) refresh_error: Option<String>,
    pub(crate) force_refresh: Option<RefreshKind>,
    pub(crate) spin_start: Option<std::time::Instant>,
}

impl UpdatesPane {
    pub fn restore_cache(&mut self) -> Task<crate::Message> {
        match pakajo::updates::load_cached() {
            Some(cache) => {
                self.last_cache = Some(cache.clone());
                self.pending = pakajo::updates::PendingUpdates {
                    repo: cache.repo.clone(),
                    aur: cache.aur.clone(),
                };
                self.count = (cache.repo.packages.len() + cache.aur.len()) as u32;
                self.state = UpdatesState::Idle;
                self.aur_error = None;
                let now = pakajo::updates::now_unix_seconds();
                let skip = !cache.repo_stale(now)
                    && !cache.devel_stale(now)
                    && pakajo::updates::localdb_unchanged_since(cache.checked_at);
                if skip {
                    eprintln!(
                        "[pakajo] updates cache fresh (repo age {}s, devel age {}s), skipping revalidation",
                        now.saturating_sub(cache.checked_at),
                        now.saturating_sub(cache.devel_checked_at)
                    );
                    Task::none()
                } else {
                    eprintln!(
                        "[pakajo] serving cached updates (repo={} aur={})",
                        cache.repo.packages.len(),
                        cache.aur.len()
                    );
                    self.start_check(RefreshKind::Launch)
                }
            }
            None => self.start_check(RefreshKind::Launch),
        }
    }

    pub fn badge(&self) -> Element<'static> {
        let (label, color_fn): (String, fn(&cosmic::Theme) -> Color) = match &self.state {
            UpdatesState::Loading => ("Checking updates...".into(), theme::muted_color),
            UpdatesState::Error(_) => ("Update check failed".into(), theme::destructive_color),
            UpdatesState::Idle if self.count > 0 => {
                (format!("{} updates", self.count), theme::on_color)
            }
            UpdatesState::Idle => ("Up to date".into(), theme::muted_color),
        };
        button::custom(text(label))
            .class(cosmic::theme::Button::Custom {
                active: theme::tinted_button(color_fn, 0.8),
                hovered: theme::tinted_button(color_fn, 1.0),
                pressed: theme::tinted_button(color_fn, 1.0),
                disabled: theme::tinted_button_disabled(color_fn, 0.8),
            })
            .on_press(crate::Message::Navigate(crate::Page::Updates))
            .into()
    }

    pub fn page(&self, sysupgrade_checking: bool) -> Element<'_> {
        let gap = cosmic::theme::spacing().space_xs as f32;
        let back = button::standard("Back").on_press(crate::Message::Navigate(crate::Page::Search));
        let refresh: Element<'_> = if self.refreshing {
            container(spinner::spinner(self.spin_start))
                .padding(8.0)
                .into()
        } else {
            button::icon(icons::refresh_cw())
                .on_press(crate::Message::Updates(UpdatesMessage::RefreshUpdates))
                .into()
        };
        let upgrade_all: Element<'_> = if sysupgrade_checking {
            button::custom(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .height(cosmic::theme::spacing().space_l)
                    .push(text("Upgrading...")),
            )
            .padding([0, cosmic::theme::spacing().space_s])
            .class(cosmic::theme::Button::Standard)
            .into()
        } else {
            let btn = button::suggested("Upgrade all");
            let btn = if self.count > 0 {
                btn.on_press(crate::Message::Sysupgrade(SysupgradeMessage::Start))
            } else {
                btn
            };
            btn.into()
        };
        let last_checked = self.last_cache.as_ref().map(|c| {
            pakajo::utils::humanize_age(
                pakajo::updates::now_unix_seconds().saturating_sub(c.checked_at),
            )
        });
        let header = Column::new()
            .spacing(8)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(12)
                    .push(back)
                    .push(Space::new().width(Length::Fill))
                    .push(muted(match &last_checked {
                        Some(age) => format!("last checked {age} ago"),
                        None => String::from("checking..."),
                    }))
                    .push(refresh)
                    .push(upgrade_all),
            )
            .push(text::title1("Updates"));
        let header = container(header)
            .width(Length::Fill)
            .padding([12.0, 16.0, 4.0, 16.0]);

        let mut content = Column::new().spacing(gap);
        match &self.state {
            UpdatesState::Loading => {
                content =
                    content.push(container(text("Checking for updates...")).padding([0.0, 16.0]));
            }
            UpdatesState::Error(msg) => {
                content = content.push(
                    container(
                        Column::new()
                            .spacing(6)
                            .push(text("Couldn't check for updates"))
                            .push(text(msg.clone()).class(Color::from_rgba(0.5, 0.5, 0.5, 1.0))),
                    )
                    .padding([0.0, 16.0]),
                );
            }
            UpdatesState::Idle => {
                if let Some(msg) = &self.refresh_error {
                    content = content.push(
                        container(destructive(format!("Update check failed: {msg}")))
                            .padding([0.0, 16.0]),
                    );
                }
                if self.count == 0 {
                    content = content
                        .push(container(text("Your system is up to date")).padding([0.0, 16.0]));
                } else {
                    let mut list = Column::new();
                    if let Some(msg) = &self.aur_error {
                        list = list.push(
                            container(destructive(format!("AUR check failed: {msg}")))
                                .padding([0.0, 16.0]),
                        );
                    }
                    if !self.pending.repo.packages.is_empty() {
                        list = list.push(band_header("Repo", self.pending.repo.packages.len()));
                        for (i, pkg) in self.pending.repo.packages.iter().enumerate() {
                            list = list.push(table_row(pkg, i));
                        }
                    }
                    if !self.pending.aur.is_empty() {
                        list = list.push(band_header("AUR", self.pending.aur.len()));
                        for (i, c) in self.pending.aur.iter().enumerate() {
                            list = list.push(aur_table_row(c, i));
                        }
                    }
                    content =
                        content.push(container(stat_cards(&self.pending)).padding([0.0, 16.0]));
                    content = content.push(list);
                }
            }
        }
        let body = container(
            container(
                scrollable(content)
                    .id(crate::page_scroll_id())
                    .direction(cosmic::iced::widget::scrollable::Direction::Vertical(
                        cosmic::iced::widget::scrollable::Scrollbar::new()
                            .width(4.0)
                            .scroller_width(4.0)
                            .spacing(0.0),
                    ))
                    .scrollbar_padding(0)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .max_width(LIST_MAX_WIDTH),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center);
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(header)
            .push(body)
            .into()
    }

    pub fn update(&mut self, message: UpdatesMessage) -> Task<crate::Message> {
        match message {
            UpdatesMessage::RefreshUpdates => self.start_check(RefreshKind::Interactive),
            UpdatesMessage::Fetched(result) => match result {
                Ok(fetch) => {
                    let count = (fetch.repo.packages.len() + fetch.aur.len()) as u32;
                    eprintln!(
                        "[pakajo] {} updates available (repo={} aur={})",
                        count,
                        fetch.repo.packages.len(),
                        fetch.aur.len()
                    );
                    self.refreshing = false;
                    self.spin_start = None;
                    self.refresh_error = None;
                    if fetch.aur_error.is_none() {
                        let now = pakajo::updates::now_unix_seconds();
                        let devel_checked_at = if fetch.devel_live {
                            now
                        } else {
                            self.last_cache
                                .as_ref()
                                .map(|c| c.devel_checked_at)
                                .unwrap_or(now)
                        };
                        let cache = pakajo::updates::UpdatesCache {
                            checked_at: now,
                            devel_checked_at,
                            repo: fetch.repo.clone(),
                            aur: fetch.aur.clone(),
                            devel: fetch.devel_names.clone(),
                        };
                        self.last_cache = Some(cache.clone());
                        pakajo::updates::store(&cache);
                    } else {
                        eprintln!("[pakajo] skipping updates cache write due to degraded fetch");
                    }
                    self.pending = pakajo::updates::PendingUpdates {
                        repo: fetch.repo,
                        aur: fetch.aur,
                    };
                    self.aur_error = fetch.aur_error;
                    self.count = count;
                    self.state = UpdatesState::Idle;
                    self.drain_pending_force_refresh()
                }
                Err(msg) => {
                    eprintln!("[pakajo] updates checker failed: {msg}");
                    self.refreshing = false;
                    self.spin_start = None;
                    if self.has_displayable_updates() {
                        self.refresh_error = Some(msg);
                    } else {
                        self.state = UpdatesState::Error(msg);
                    }
                    self.drain_pending_force_refresh()
                }
            },
        }
    }

    fn drain_pending_force_refresh(&mut self) -> Task<crate::Message> {
        match self.force_refresh.take() {
            Some(kind) => self.start_check(kind),
            None => Task::none(),
        }
    }

    fn has_displayable_updates(&self) -> bool {
        self.last_cache.is_some() || !self.pending.repo.is_empty() || !self.pending.aur.is_empty()
    }

    pub fn start_check(&mut self, kind: RefreshKind) -> Task<crate::Message> {
        if self.refreshing {
            self.force_refresh = match self.force_refresh {
                Some(existing) if existing >= kind => Some(existing),
                _ => Some(kind),
            };
            return Task::none();
        }
        self.refreshing = true;
        self.spin_start = Some(std::time::Instant::now());
        let now = pakajo::updates::now_unix_seconds();
        let devel_source = match (kind, self.last_cache.as_ref()) {
            (RefreshKind::Launch, Some(cache)) if !cache.devel_stale(now) => {
                let age = now.saturating_sub(cache.devel_checked_at);
                eprintln!("[pakajo] devel updates from cache (age {age}s)");
                pakajo::upgrade::DevelSource::Cached(cache.devel.clone())
            }
            _ => {
                eprintln!("[pakajo] devel updates live");
                pakajo::upgrade::DevelSource::Live
            }
        };
        if !self.has_displayable_updates() {
            self.state = UpdatesState::Loading;
        }
        crate::components::task::blocking_task(
            move || pakajo::updates::pending_updates(devel_source),
            "updates check cancelled",
            |result| crate::Message::Updates(UpdatesMessage::Fetched(result)).into(),
        )
    }
}
