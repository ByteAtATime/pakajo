use cosmic::app::Task;
use cosmic::iced::{Alignment, Color, Length};
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};

use pakajo::updates::RepoUpgrade;
use pakajo::upgrade::AurUpgradeCandidate;

use crate::Element;
use crate::components::sysupgrade::SysupgradeMessage;
use crate::components::theme;
use crate::components::theme::{muted_mono, muted_text as muted, themed_mono_text, themed_text};

#[derive(Clone, Debug)]
pub enum UpdatesMessage {
    RefreshUpdates,
    Fetched(Result<pakajo::updates::UpdatesFetch, String>),
}

#[derive(Clone, Debug)]
pub enum UpdatesState {
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

pub enum UpdatesEntry<'a> {
    Header(String),
    Repo(&'a RepoUpgrade),
    Aur(&'a AurUpgradeCandidate),
}

pub fn build_updates_items(updates: &pakajo::updates::PendingUpdates) -> Vec<UpdatesEntry<'_>> {
    let mut items: Vec<UpdatesEntry<'_>> = Vec::new();
    if !updates.repo.is_empty() {
        items.push(UpdatesEntry::Header(format!(
            "Repository ({})",
            updates.repo.len()
        )));
        items.extend(updates.repo.iter().map(UpdatesEntry::Repo));
    }
    if !updates.aur.is_empty() {
        items.push(UpdatesEntry::Header(format!("AUR ({})", updates.aur.len())));
        items.extend(updates.aur.iter().map(UpdatesEntry::Aur));
    }
    items
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

pub fn repo_upgrade_row(r: &RepoUpgrade) -> Element<'_> {
    let left = Row::new()
        .spacing(6)
        .push(crate::components::row_title(r.name.clone()))
        .push(muted(&r.repo));
    let right = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(colored_version_delta(&r.old, &r.new))
        .push(muted(pakajo::utils::format_bytes(r.download_size)));
    Row::new()
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(12)
        .push(left)
        .push(Space::new().width(Length::Fill))
        .push(right)
        .into()
}

pub fn aur_upgrade_row(c: &AurUpgradeCandidate) -> Element<'_> {
    Row::new()
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(12)
        .push(crate::components::row_title(c.name.clone()))
        .push(Space::new().width(Length::Fill))
        .push(aur_version_delta(c))
        .into()
}

pub fn updates_section_header(title: &str) -> Element<'static> {
    Column::new()
        .spacing(4)
        .push(Space::new().height(16.0))
        .push(text::heading(title.to_string()))
        .into()
}

impl crate::PakajoApp {
    pub(crate) fn updates_badge(&self) -> Element<'static> {
        let (label, color_fn): (String, fn(&cosmic::Theme) -> Color) = match &self.updates_state {
            UpdatesState::Loading => ("Checking updates...".into(), theme::muted_color),
            UpdatesState::Error(_) => ("Update check failed".into(), theme::destructive_color),
            UpdatesState::Idle if self.pending_count > 0 => {
                (format!("{} updates", self.pending_count), theme::on_color)
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

    pub(crate) fn updates_page(&self) -> Element<'_> {
        let back = button::standard("Back").on_press(crate::Message::Navigate(crate::Page::Search));
        let refresh: Element<'_> = if self.updates_refreshing {
            text("Refreshing...").into()
        } else {
            button::standard("Refresh")
                .on_press(crate::Message::Updates(UpdatesMessage::RefreshUpdates))
                .into()
        };
        let upgrade_all: Element<'_> = if self.sysupgrade_preview_in_flight {
            text("Checking...").into()
        } else {
            let btn = button::standard("Upgrade all");
            let btn = if self.pending_count > 0 {
                btn.on_press(crate::Message::Sysupgrade(SysupgradeMessage::StartPreview))
            } else {
                btn
            };
            btn.into()
        };
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Updates"))
            .push(Space::new().width(Length::Fill))
            .push(upgrade_all)
            .push(refresh);
        let padded_header = container(header).padding([12.0, 12.0]);
        let body: Element<'_> = match &self.updates_state {
            UpdatesState::Loading => container(text("Checking for updates..."))
                .padding([0.0, 12.0])
                .into(),
            UpdatesState::Error(msg) => container(
                Column::new()
                    .spacing(6)
                    .push(text("Couldn't check for updates"))
                    .push(text(msg.clone()).class(Color::from_rgba(0.5, 0.5, 0.5, 1.0))),
            )
            .padding([0.0, 12.0])
            .into(),
            UpdatesState::Idle => {
                let mut body = Column::new().padding([0.0, 12.0]).spacing(16);
                if let Some(msg) = &self.updates_refresh_error {
                    body = body.push(destructive(format!("Update check failed: {msg}")));
                }
                if self.pending_count == 0 {
                    body = body.push(text("Your system is up to date"));
                } else {
                    let mut list = Column::new().spacing(16);
                    if let Some(msg) = &self.updates_aur_error {
                        list = list.push(destructive(format!("AUR check failed: {msg}")));
                    }
                    for entry in build_updates_items(&self.pending_updates) {
                        match entry {
                            UpdatesEntry::Header(title) => {
                                list = list.push(updates_section_header(&title));
                            }
                            UpdatesEntry::Repo(r) => list = list.push(repo_upgrade_row(r)),
                            UpdatesEntry::Aur(c) => list = list.push(aur_upgrade_row(c)),
                        }
                    }
                    body = body.push(
                        scrollable(list)
                            .id(crate::page_scroll_id())
                            .width(Length::Fill)
                            .height(Length::Fill),
                    );
                }
                body.into()
            }
        };
        let last_checked: Option<String> = self.last_cache.as_ref().map(|c| {
            let now = pakajo::updates::now_unix_seconds();
            pakajo::utils::humanize_age(now.saturating_sub(c.checked_at))
        });
        let last_checked_line: Option<Element<'_>> =
            last_checked.map(|age| muted(format!("Last checked {age}")));
        let mut column = Column::new().push(padded_header);
        if let Some(line) = last_checked_line {
            column = column.push(line);
        }
        column = column.push(body);
        container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn handle_updates(&mut self, message: UpdatesMessage) -> Task<crate::Message> {
        match message {
            UpdatesMessage::RefreshUpdates => self.start_updates_check(RefreshKind::Interactive),
            UpdatesMessage::Fetched(result) => match result {
                Ok(fetch) => {
                    let count = (fetch.repo.len() + fetch.aur.len()) as u32;
                    eprintln!(
                        "[pakajo] {} updates available (repo={} aur={})",
                        count,
                        fetch.repo.len(),
                        fetch.aur.len()
                    );
                    self.updates_refreshing = false;
                    self.updates_refresh_error = None;
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
                    self.pending_updates = pakajo::updates::PendingUpdates {
                        repo: fetch.repo,
                        aur: fetch.aur,
                    };
                    self.updates_aur_error = fetch.aur_error;
                    self.pending_count = count;
                    self.updates_state = UpdatesState::Idle;
                    self.drain_pending_force_refresh()
                }
                Err(msg) => {
                    eprintln!("[pakajo] updates checker failed: {msg}");
                    self.updates_refreshing = false;
                    if self.has_displayable_updates() {
                        self.updates_refresh_error = Some(msg);
                    } else {
                        self.updates_state = UpdatesState::Error(msg);
                    }
                    self.drain_pending_force_refresh()
                }
            },
        }
    }

    fn drain_pending_force_refresh(&mut self) -> Task<crate::Message> {
        match self.pending_force_refresh.take() {
            Some(kind) => self.start_updates_check(kind),
            None => Task::none(),
        }
    }

    fn has_displayable_updates(&self) -> bool {
        self.last_cache.is_some()
            || !self.pending_updates.repo.is_empty()
            || !self.pending_updates.aur.is_empty()
    }

    pub(crate) fn start_updates_check(&mut self, kind: RefreshKind) -> Task<crate::Message> {
        if self.updates_refreshing {
            self.pending_force_refresh = match self.pending_force_refresh {
                Some(existing) if existing >= kind => Some(existing),
                _ => Some(kind),
            };
            return Task::none();
        }
        self.updates_refreshing = true;
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
            self.updates_state = UpdatesState::Loading;
        }
        crate::components::task::blocking_task(
            move || pakajo::updates::pending_updates(devel_source),
            "updates check cancelled",
            |result| crate::Message::Updates(UpdatesMessage::Fetched(result)).into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_updates_items_groups_repo_then_aur_and_is_empty_when_blank() {
        let pending = pakajo::updates::PendingUpdates {
            repo: vec![
                RepoUpgrade {
                    name: "alpha".to_string(),
                    old: "1.0".to_string(),
                    new: "1.1".to_string(),
                    download_size: 1024,
                    repo: "core".to_string(),
                },
                RepoUpgrade {
                    name: "beta".to_string(),
                    old: "2.0".to_string(),
                    new: "2.1".to_string(),
                    download_size: 0,
                    repo: "extra".to_string(),
                },
            ],
            aur: vec![AurUpgradeCandidate {
                name: "aur-pkg".to_string(),
                local_version: "0.1".to_string(),
                remote_version: "0.2".to_string(),
                package_base: "aur-pkg".to_string(),
            }],
        };
        let items = build_updates_items(&pending);
        assert_eq!(items.len(), 5);
        assert!(matches!(&items[0], UpdatesEntry::Header(h) if h.as_str() == "Repository (2)"));
        assert!(matches!(&items[1], UpdatesEntry::Repo(_)));
        assert!(matches!(&items[2], UpdatesEntry::Repo(_)));
        assert!(matches!(&items[3], UpdatesEntry::Header(h) if h.as_str() == "AUR (1)"));
        assert!(matches!(&items[4], UpdatesEntry::Aur(_)));

        let blank = pakajo::updates::PendingUpdates {
            repo: Vec::new(),
            aur: Vec::new(),
        };
        assert!(build_updates_items(&blank).is_empty());
    }
}
