use cosmic::app::Task;
use cosmic::widget::{Column, Row, button, checkbox, container, radio, scrollable, space, text};

use anyhow::Context as _;
use pakajo::dry_run::SysupgradePreview;
use pakajo::transaction_state::{Direction, SysupgradePage, next_sysupgrade_step};

use crate::transaction::review::{ReviewModel, candidate_label, unsupported_banner};

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SysupgradeMessage {
    StartPreview,
    PreviewFetched(Result<SysupgradePreview, String>),
    Abort,
    ToggleConflict(usize),
    SelectProvider { depend: String, idx: usize },
    Continue,
    Back,
}

fn sysupgrade_page_to_view(p: SysupgradePage) -> crate::Page {
    match p {
        SysupgradePage::Updates => crate::Page::Updates,
        SysupgradePage::Resolve => crate::Page::Resolve,
        SysupgradePage::PkgbuildReview => crate::Page::PkgbuildReview,
        SysupgradePage::Confirm => crate::Page::Confirm,
    }
}

fn view_to_sysupgrade_page(page: &crate::Page) -> Option<SysupgradePage> {
    match page {
        crate::Page::Resolve => Some(SysupgradePage::Resolve),
        crate::Page::PkgbuildReview => Some(SysupgradePage::PkgbuildReview),
        crate::Page::Confirm => Some(SysupgradePage::Confirm),
        _ => None,
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
                        self.sysupgrade_review = if has_resolve {
                            Some(ReviewModel::new(preview.questions.clone()))
                        } else {
                            None
                        };
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
                        self.goto_page(sysupgrade_page_to_view(next))
                    }
                    Err(msg) => {
                        self.sysupgrade_preview = None;
                        self.sysupgrade_preview_error = Some(msg.clone());
                        self.sysupgrade_review = None;
                        eprintln!("[pakajo] sysupgrade preview failed: {msg}");
                        Task::none()
                    }
                }
            }
            SysupgradeMessage::Abort => {
                self.sysupgrade_preview = None;
                self.sysupgrade_preview_error = None;
                self.sysupgrade_aur_targets.clear();
                self.sysupgrade_review = None;
                self.goto_page(crate::Page::Updates)
            }
            SysupgradeMessage::ToggleConflict(i) => {
                if let Some(r) = self.sysupgrade_review.as_mut() {
                    r.toggle_conflict(i);
                }
                Task::none()
            }
            SysupgradeMessage::SelectProvider { depend, idx } => {
                if let Some(r) = self.sysupgrade_review.as_mut() {
                    r.select_provider(depend, idx);
                }
                Task::none()
            }
            SysupgradeMessage::Continue => self.route_sysupgrade(Direction::Forward),
            SysupgradeMessage::Back => self.route_sysupgrade(Direction::Backward),
        }
    }

    fn route_sysupgrade(&mut self, dir: Direction) -> Task<crate::Message> {
        let Some(from) = view_to_sysupgrade_page(&self.page) else {
            return Task::none();
        };
        let has_resolve = self.sysupgrade_review.is_some();
        let has_diffs = self
            .sysupgrade_preview
            .as_ref()
            .map(|p| !p.pkgbuild_diffs.is_empty())
            .unwrap_or(false);
        let next = next_sysupgrade_step(from, dir, has_resolve, has_diffs);
        eprintln!(
            "[pakajo] sysupgrade route {:?} {:?} -> {:?}",
            from, dir, next
        );
        self.goto_page(sysupgrade_page_to_view(next))
    }

    pub(crate) fn resolve_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back = button::custom(text("Back"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));
        let continue_btn = button::custom(text("Continue"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Continue));
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Resolve conflicts"))
            .push(space::horizontal())
            .push(continue_btn);
        let padded_header = container(header).padding([12.0, 12.0]);

        let mut body = Column::new().spacing(16).padding([0.0, 12.0]);

        if let Some(r) = &self.sysupgrade_review {
            if !r.qs.conflicts.is_empty() {
                body = body.push(text("Conflicts"));
                for (i, conflict) in r.qs.conflicts.iter().enumerate() {
                    let label = format!("Replace {} with {}", conflict.removable, conflict.incoming);
                    let checked = r.conflict_checks.get(i).copied().unwrap_or(false);
                    let item = checkbox(checked).label(label).on_toggle(move |_| {
                        crate::Message::Sysupgrade(SysupgradeMessage::ToggleConflict(i))
                    });
                    body = body.push(item);
                }
            }

            if !r.qs.providers.is_empty() {
                body = body.push(text("Providers"));
                for prompt in &r.qs.providers {
                    body = body.push(text(prompt.depend.clone()));
                    let selected = r
                        .provider_choices
                        .get(&prompt.depend)
                        .copied()
                        .unwrap_or(0);
                    for (idx, candidate) in prompt.candidates.iter().enumerate() {
                        let label = candidate_label(candidate);
                        let depend = prompt.depend.clone();
                        let item = radio(text(label), idx, Some(selected), move |chosen: usize| {
                            crate::Message::Sysupgrade(SysupgradeMessage::SelectProvider {
                                depend: depend.clone(),
                                idx: chosen,
                            })
                        });
                        body = body.push(item);
                    }
                }
            }

            if r.qs.had_unsupported_question {
                body = body.push(unsupported_banner(&r.qs.unsupported_summary));
            }
        }

        let column = Column::new()
            .push(padded_header)
            .push(scrollable(body).id(crate::page_scroll_id()));
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
