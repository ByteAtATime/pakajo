use cosmic::app::Task;
use cosmic::widget::{Column, Row, button, container, text};

use anyhow::Context as _;
use pakajo::dry_run::SysupgradePreview;
use pakajo::transaction_state::{Direction, SysupgradePage, next_sysupgrade_step};

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SysupgradeMessage {
    StartPreview,
    PreviewFetched(Result<SysupgradePreview, String>),
    Abort,
}

fn sysupgrade_page_to_view(p: SysupgradePage) -> crate::Page {
    match p {
        SysupgradePage::Updates => crate::Page::Updates,
        SysupgradePage::Resolve => crate::Page::Resolve,
        SysupgradePage::PkgbuildReview => crate::Page::PkgbuildReview,
        SysupgradePage::Confirm => crate::Page::Confirm,
    }
}

impl crate::PakajoApp {
    pub(crate) fn start_sysupgrade_preview(&mut self) -> Task<crate::Message> {
        if self.transaction.as_ref().is_some_and(|t| t.is_active()) {
            return Task::none();
        }
        if self.sysupgrade_preview_in_flight {
            return Task::none();
        }
        self.sysupgrade_preview_in_flight = true;
        eprintln!("[pakajo] starting sysupgrade preview");
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<SysupgradePreview> {
                let config = pacmanconf::Config::new().context("failed to read pacman config")?;
                let mut handle = pakajo::pacman::init_alpm_rootless(&config)?;
                handle
                    .syncdbs_mut()
                    .update(false)
                    .context("failed to refresh checkdb sync DBs rootless")?;
                let mut preview = pakajo::dry_run::compute_sysupgrade_preview(&mut handle, &config)?;
                let aur_names: Vec<String> = preview.aur.iter().map(|c| c.name.clone()).collect();
                if !aur_names.is_empty() {
                    match pakajo::pkgbuild::prepare_pkgbuild_diffs(&aur_names) {
                        Ok(diffs) => preview.pkgbuild_diffs = diffs,
                        Err(e) => eprintln!("[pakajo] pkgbuild diff computation failed: {e:#}"),
                    }
                }
                Ok(preview)
            })();
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
        });
        Task::perform(
            async move {
                match rx.await {
                    Ok(Ok(p)) => Ok(p),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("sysupgrade preview cancelled".to_string()),
                }
            },
            |result| crate::Message::Sysupgrade(SysupgradeMessage::PreviewFetched(result)).into(),
        )
    }

    pub(crate) fn handle_sysupgrade(&mut self, message: SysupgradeMessage) -> Task<crate::Message> {
        match message {
            SysupgradeMessage::StartPreview => self.start_sysupgrade_preview(),
            SysupgradeMessage::PreviewFetched(result) => {
                self.sysupgrade_preview_in_flight = false;
                match result {
                    Ok(preview) => {
                        self.sysupgrade_aur_targets =
                            preview.aur.iter().map(|c| c.name.clone()).collect();
                        self.sysupgrade_preview = Some(preview.clone());
                        self.sysupgrade_preview_error = None;
                        let has_resolve = !preview.questions.conflicts.is_empty()
                            || !preview.questions.providers.is_empty();
                        let has_diffs = !preview.pkgbuild_diffs.is_empty();
                        let next = next_sysupgrade_step(
                            SysupgradePage::Updates,
                            Direction::Forward,
                            has_resolve,
                            has_diffs,
                        );
                        eprintln!(
                            "[pakajo] sysupgrade preview ready (repo={} aur={} resolve={} diffs={}) -> {:?}",
                            preview.summary.packages.len(),
                            self.sysupgrade_aur_targets.len(),
                            has_resolve,
                            has_diffs,
                            next
                        );
                        self.page = sysupgrade_page_to_view(next);
                        Task::none()
                    }
                    Err(msg) => {
                        self.sysupgrade_preview = None;
                        self.sysupgrade_preview_error = Some(msg.clone());
                        eprintln!("[pakajo] sysupgrade preview failed: {msg}");
                        Task::none()
                    }
                }
            }
            SysupgradeMessage::Abort => {
                self.sysupgrade_preview = None;
                self.sysupgrade_preview_error = None;
                self.sysupgrade_aur_targets.clear();
                self.page = crate::Page::Updates;
                Task::none()
            }
        }
    }

    pub(crate) fn resolve_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back = button::custom(text("Back"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Abort));
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Resolve conflicts"));
        let padded_header = container(header).padding([12.0, 12.0]);
        let column = Column::new().push(padded_header);
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }

    pub(crate) fn pkgbuild_review_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back = button::custom(text("Back"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Abort));
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Review PKGBUILD"));
        let padded_header = container(header).padding([12.0, 12.0]);
        let column = Column::new().push(padded_header);
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }

    pub(crate) fn confirm_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back = button::custom(text("Back"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Abort));
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Confirm upgrade"));
        let padded_header = container(header).padding([12.0, 12.0]);
        let column = Column::new().push(padded_header);
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }
}
