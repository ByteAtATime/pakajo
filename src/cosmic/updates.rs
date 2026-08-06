use cosmic::app::Task;
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};

use pakajo::updates::RepoUpgrade;
use pakajo::upgrade::AurUpgradeCandidate;

use crate::sysupgrade::SysupgradeMessage;

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

pub fn muted_text_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    let mut on = theme.cosmic().background(false).on;
    on.alpha = 0.7;
    cosmic::iced::widget::text::Style {
        color: Some(on.into()),
        ..Default::default()
    }
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

pub fn themed_text<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    style_fn: fn(&cosmic::Theme) -> cosmic::iced::widget::text::Style,
) -> cosmic::Element<'a, crate::Message> {
    text(content)
        .class(cosmic::theme::Text::Custom(style_fn))
        .into()
}

pub fn muted<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::Element<'a, crate::Message> {
    themed_text(content, muted_text_style)
}

pub fn destructive<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::Element<'a, crate::Message> {
    themed_text(content, destructive_text_style)
}

pub fn success<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::Element<'a, crate::Message> {
    themed_text(content, success_text_style)
}

pub fn colored_version_delta(old: &str, new: &str) -> cosmic::Element<'static, crate::Message> {
    let (common, old_suffix, new_suffix) = pakajo::utils::version_diff(old, new);
    Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(4)
        .push(
            Row::new()
                .push(muted(common.clone()))
                .push(destructive(old_suffix)),
        )
        .push(text(" \u{2192} "))
        .push(Row::new().push(muted(common)).push(success(new_suffix)))
        .into()
}

pub fn aur_version_delta<'a>(
    candidate: &'a AurUpgradeCandidate,
) -> cosmic::Element<'a, crate::Message> {
    if candidate.remote_version == "latest-commit" {
        Row::new()
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(4)
            .push(success(&candidate.local_version))
            .push(text(" \u{2192} "))
            .push(success("latest-commit"))
            .into()
    } else {
        colored_version_delta(&candidate.local_version, &candidate.remote_version)
    }
}

pub fn repo_upgrade_row(r: &RepoUpgrade) -> cosmic::Element<'_, crate::Message> {
    let left = Row::new()
        .spacing(6)
        .push(text(&r.name).font(cosmic::font::semibold()))
        .push(muted(&r.repo));
    let right = Row::new()
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .push(colored_version_delta(&r.old, &r.new))
        .push(muted(pakajo::utils::format_bytes(r.download_size)));
    Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .width(cosmic::iced::Length::Fill)
        .spacing(12)
        .push(left)
        .push(Space::new().width(cosmic::iced::Length::Fill))
        .push(right)
        .into()
}

pub fn aur_upgrade_row(c: &AurUpgradeCandidate) -> cosmic::Element<'_, crate::Message> {
    Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .width(cosmic::iced::Length::Fill)
        .spacing(12)
        .push(text(&c.name).font(cosmic::font::semibold()))
        .push(Space::new().width(cosmic::iced::Length::Fill))
        .push(aur_version_delta(c))
        .into()
}

pub fn updates_section_header(title: &str) -> cosmic::Element<'static, crate::Message> {
    Column::new()
        .spacing(4)
        .push(Space::new().height(16.0))
        .push(text(title.to_string()).font(cosmic::font::semibold()))
        .into()
}

impl crate::PakajoApp {
    pub(crate) fn updates_badge(&self) -> cosmic::Element<'_, crate::Message> {
        let (label, class) = match &self.updates_state {
            UpdatesState::Loading => ("...".to_string(), cosmic::theme::Button::Standard),
            UpdatesState::Error(_) => ("!".to_string(), cosmic::theme::Button::Destructive),
            UpdatesState::Idle => {
                if self.pending_count > 0 {
                    (
                        self.pending_count.to_string(),
                        cosmic::theme::Button::Suggested,
                    )
                } else {
                    (
                        self.pending_count.to_string(),
                        cosmic::theme::Button::Standard,
                    )
                }
            }
        };

        button::custom(text(label))
            .on_press(crate::Message::Navigate(crate::Page::Updates))
            .class(class)
            .into()
    }

    pub(crate) fn updates_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back =
            button::custom(text("Back")).on_press(crate::Message::Navigate(crate::Page::Search));
        let refresh: cosmic::Element<'_, crate::Message> =
            if matches!(self.updates_state, UpdatesState::Loading) {
                text("Refreshing...").into()
            } else {
                button::custom(text("Refresh"))
                    .on_press(crate::Message::Updates(UpdatesMessage::RefreshUpdates))
                    .into()
            };
        let upgrade_all: cosmic::Element<'_, crate::Message> =
            if self.sysupgrade_preview_in_flight {
                text("Checking...").into()
            } else {
                let btn = button::custom(text("Upgrade all"));
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
            .push(Space::new().width(cosmic::iced::Length::Fill))
            .push(upgrade_all)
            .push(refresh);
        let padded_header = container(header).padding([12.0, 12.0]);
        let body: cosmic::Element<'_, crate::Message> = match &self.updates_state {
            UpdatesState::Loading => container(text("Checking for updates..."))
                .padding([0.0, 12.0])
                .into(),
            UpdatesState::Error(msg) => container(
                Column::new()
                    .spacing(6)
                    .push(text("Couldn't check for updates"))
                    .push(
                        text(msg.clone())
                            .class(cosmic::iced::Color::from_rgba(0.5, 0.5, 0.5, 1.0)),
                    ),
            )
            .padding([0.0, 12.0])
            .into(),
            UpdatesState::Idle => {
                if self.pending_count == 0 {
                    container(text("Your system is up to date"))
                        .padding([0.0, 12.0])
                        .into()
                } else {
                    let mut list = Column::new().padding([0.0, 12.0]).spacing(16);
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
                    scrollable(list)
                        .id(crate::page_scroll_id())
                        .width(cosmic::iced::Length::Fill)
                        .height(cosmic::iced::Length::Fill)
                        .into()
                }
            }
        };
        let column = Column::new().push(padded_header).push(body);
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }

    pub(crate) fn handle_updates(
        &mut self,
        message: UpdatesMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            UpdatesMessage::RefreshUpdates => self.start_updates_check(),
            UpdatesMessage::Fetched(result) => match result {
                Ok(fetch) => {
                    let count = (fetch.repo.len() + fetch.aur.len()) as u32;
                    eprintln!(
                        "[pakajo] {} updates available (repo={} aur={})",
                        count,
                        fetch.repo.len(),
                        fetch.aur.len()
                    );
                    self.pending_updates = pakajo::updates::PendingUpdates {
                        repo: fetch.repo,
                        aur: fetch.aur,
                    };
                    self.updates_aur_error = fetch.aur_error;
                    self.pending_count = count;
                    self.updates_state = UpdatesState::Idle;
                    Task::none()
                }
                Err(msg) => {
                    eprintln!("[pakajo] updates checker failed: {msg}");
                    self.updates_state = UpdatesState::Error(msg);
                    Task::none()
                }
            },
        }
    }

    pub(crate) fn start_updates_check(&mut self) -> cosmic::app::Task<crate::Message> {
        if matches!(self.updates_state, UpdatesState::Loading) {
            return Task::none();
        }
        self.updates_state = UpdatesState::Loading;
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let result = pakajo::updates::pending_updates();
            let _ = tx.send(result);
        });
        Task::perform(
            async move {
                match rx.await {
                    Ok(Ok(fetch)) => Ok(fetch),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("updates check cancelled".to_string()),
                }
            },
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
