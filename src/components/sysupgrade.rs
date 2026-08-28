use cosmic::app::Task;
use cosmic::iced::Color;
use cosmic::widget::{Column, Row, button, checkbox, container, radio, scrollable, space, text};

use anyhow::Context as _;
use pakajo::dry_run::{PrepareFailure, SysupgradePreview};
use pakajo::events::{SummaryPackage, TransactionSummary};
use pakajo::question::{collect_approvals, default_approve, encode_approvals};
use pakajo::transaction_state::{Direction, SysupgradePage, next_sysupgrade_step};

use crate::components::updates::{aur_upgrade_row, muted};
use crate::components::transaction::review::{ReviewModel, candidate_label, unsupported_banner};
use crate::components::transaction::Transaction;

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SysupgradeMessage {
    StartPreview,
    PreviewFetched(Result<SysupgradePreview, String>),
    ToggleConflict(usize),
    SelectProvider { depend: String, idx: usize },
    Continue,
    Back,
    Apply,
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

fn no_preview() -> cosmic::Element<'static, crate::Message> {
    container(text("No preview available"))
        .width(cosmic::iced::Length::Fill)
        .height(cosmic::iced::Length::Fill)
        .into()
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

    pub(crate) fn start_sysupgrade_apply(&mut self) -> Task<crate::Message> {
        if self.transaction.as_ref().is_some_and(|t| t.is_active()) {
            return Task::none();
        }
        let Some(preview) = &self.sysupgrade_preview else {
            return Task::none();
        };
        if preview.prepare_error.is_some() {
            return Task::none();
        }
        let summary_bytes = serde_json::to_vec(&preview.summary).unwrap_or_default();
        let fingerprint_file = match pakajo::upgrade::write_fingerprint_file(&summary_bytes) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("[pakajo] fingerprint write failed: {e}");
                return Task::none();
            }
        };
        let approvals_b64 = {
            let collected = if let Some(r) = &self.sysupgrade_review {
                collect_approvals(&r.qs, &r.conflict_checks, &r.provider_choices)
            } else {
                default_approve(&preview.questions)
            };
            match collected {
                Ok(approvals) => match encode_approvals(&approvals) {
                    Ok(b64) => Some(b64),
                    Err(e) => {
                        eprintln!("[pakajo] approval encoding failed: {e}");
                        None
                    }
                },
                Err(e) => {
                    eprintln!("[pakajo] approval collection failed: {e}");
                    None
                }
            }
        };
        let (transaction, task) = Transaction::start_sysupgrade_repo(fingerprint_file, approvals_b64);
        self.transaction = Some(transaction);
        self.active_sysupgrade_phase = Some(pakajo::transaction_state::SysupgradePhase::Repo);
        eprintln!("[pakajo] sysupgrade repo apply started");
        task
    }

    pub(crate) fn handle_sysupgrade(&mut self, message: SysupgradeMessage) -> Task<crate::Message> {
        match message {
            SysupgradeMessage::StartPreview => self.start_sysupgrade_preview(),
            SysupgradeMessage::Apply => self.start_sysupgrade_apply(),
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
                        if matches!(next, SysupgradePage::PkgbuildReview) {
                            self.pkgbuild_review_index = 0;
                        }
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
                        self.pkgbuild_review_index = 0;
                        eprintln!("[pakajo] sysupgrade preview failed: {msg}");
                        Task::none()
                    }
                }
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
            SysupgradeMessage::Continue => {
                if matches!(self.page, crate::Page::PkgbuildReview) {
                    let diffs_len = self
                        .sysupgrade_preview
                        .as_ref()
                        .map(|p| p.pkgbuild_diffs.len())
                        .unwrap_or(0);
                    if self.pkgbuild_review_index + 1 < diffs_len {
                        self.pkgbuild_review_index += 1;
                        return crate::scroll_to_top();
                    }
                }
                self.route_sysupgrade(Direction::Forward)
            }
            SysupgradeMessage::Back => {
                if matches!(self.page, crate::Page::PkgbuildReview)
                    && self.pkgbuild_review_index > 0
                {
                    self.pkgbuild_review_index -= 1;
                    return crate::scroll_to_top();
                }
                self.route_sysupgrade(Direction::Backward)
            }
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
        if matches!(next, SysupgradePage::PkgbuildReview) {
            self.pkgbuild_review_index = 0;
        }
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
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));
        let continue_btn = button::custom(text("Continue"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Continue));
        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Review PKGBUILD"))
            .push(space::horizontal())
            .push(continue_btn);
        let padded_header = container(header).padding([12.0, 12.0]);

        let Some(preview) = self.sysupgrade_preview.as_ref() else {
            return no_preview();
        };
        let diffs = &preview.pkgbuild_diffs;
        if diffs.is_empty() {
            return no_preview();
        }
        let index = self.pkgbuild_review_index.min(diffs.len() - 1);
        let current = &diffs[index];

        let position = format!("Diff {} of {}", index + 1, diffs.len());
        let body = Column::new()
            .spacing(16)
            .padding([0.0, 12.0])
            .push(text(position))
            .push(crate::components::transaction::diff_lines_column(current));

        let column = Column::new()
            .push(padded_header)
            .push(scrollable(body).id(crate::page_scroll_id()));
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }

    pub(crate) fn confirm_page(&self) -> cosmic::Element<'_, crate::Message> {
        let back = button::custom(text("Back"))
            .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));

        let apply_disabled = match self.sysupgrade_preview.as_ref() {
            Some(preview) => {
                preview.prepare_error.is_some()
                    || preview.summary.packages.is_empty()
                    || self.transaction.as_ref().is_some_and(|t| t.is_active())
                    || self
                        .sysupgrade_review
                        .as_ref()
                        .map(|r| r.conflict_checks.iter().any(|&c| !c))
                        .unwrap_or_else(|| !preview.questions.conflicts.is_empty())
            }
            None => true,
        };
        let apply = if apply_disabled {
            button::custom(text("Apply")).class(cosmic::theme::Button::Suggested)
        } else {
            button::custom(text("Apply"))
                .class(cosmic::theme::Button::Suggested)
                .on_press(crate::Message::Sysupgrade(SysupgradeMessage::Apply))
        };

        let header = Row::new()
            .spacing(12)
            .push(back)
            .push(text("Review upgrade"))
            .push(space::horizontal())
            .push(apply);
        let padded_header = container(header).padding([12.0, 12.0]);

        let Some(preview) = self.sysupgrade_preview.as_ref() else {
            return container(Column::new().push(padded_header).push(no_preview()))
                .width(cosmic::iced::Length::Fill)
                .height(cosmic::iced::Length::Fill)
                .into();
        };

        let mut body = Column::new().spacing(16).padding([0.0, 12.0]);

        if let Some(failure) = &preview.prepare_error {
            body = body.push(blocked_banner(failure));
        }

        body = body.push(manifest_section(&preview.summary));

        if !preview.aur.is_empty() {
            let mut aur_col = Column::new().spacing(8);
            aur_col = aur_col.push(
                text(format!("AUR packages to build ({})", preview.aur.len()))
                    .font(cosmic::font::semibold()),
            );
            for candidate in &preview.aur {
                aur_col = aur_col.push(aur_upgrade_row(candidate));
            }
            body = body.push(aur_col);
        }

        let column = Column::new()
            .push(padded_header)
            .push(scrollable(body).id(crate::page_scroll_id()));
        container(column)
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }
}

fn accent_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().accent.base)
}

fn divider() -> cosmic::Element<'static, crate::Message> {
    container(text(""))
        .width(cosmic::iced::Length::Fill)
        .height(1.0)
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(cosmic::iced::Background::Color(Color::from(
                theme.cosmic().background(false).divider,
            ))),
            ..Default::default()
        })
        .into()
}

fn success_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().success.base)
}

fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

fn op_pill(label: &str, color_fn: fn(&cosmic::Theme) -> Color) -> cosmic::Element<'static, crate::Message> {
    container(text(label.to_string()))
        .padding([2.0, 8.0])
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(color_fn(theme)),
            background: Some(cosmic::iced::Background::Color({
                let colored = color_fn(theme);
                Color { a: 0.10, ..colored }
            })),
            ..Default::default()
        })
        .into()
}

fn summary_row(pkg: &SummaryPackage) -> cosmic::Element<'_, crate::Message> {
    let (op_label, color_fn, version_text) = if pkg.is_removal {
        (
            "Remove",
            destructive_color as fn(&cosmic::Theme) -> Color,
            pkg.old_version.clone().unwrap_or_default(),
        )
    } else if pkg.old_version.is_some() {
        (
            "Upgrade",
            accent_color as fn(&cosmic::Theme) -> Color,
            format!(
                "{} \u{2192} {}",
                pkg.old_version.as_deref().unwrap_or("?"),
                pkg.new_version
            ),
        )
    } else {
        (
            "Install",
            success_color as fn(&cosmic::Theme) -> Color,
            pkg.new_version.clone(),
        )
    };

    let left = text(&pkg.name);
    let right = Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(8)
        .push(muted(version_text))
        .push(op_pill(op_label, color_fn));
    Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .width(cosmic::iced::Length::Fill)
        .spacing(12)
        .push(left)
        .push(space::horizontal())
        .push(right)
        .into()
}

fn manifest_section(summary: &TransactionSummary) -> cosmic::Element<'_, crate::Message> {
    let mut installs = 0u32;
    let mut upgrades = 0u32;
    let mut removes = 0u32;
    for pkg in &summary.packages {
        if pkg.is_removal {
            removes += 1;
        } else if pkg.old_version.is_some() {
            upgrades += 1;
        } else {
            installs += 1;
        }
    }

    let mut counts: Vec<String> = Vec::new();
    if installs > 0 {
        counts.push(format!("Install {installs}"));
    }
    if upgrades > 0 {
        counts.push(format!("Upgrade {upgrades}"));
    }
    if removes > 0 {
        counts.push(format!("Remove {removes}"));
    }
    let counts_left = counts.join(" \u{00b7} ");

    let download_right = if summary.total_download_size > 0 {
        Some(format!(
            "\u{2193} {}",
            pakajo::utils::format_bytes(summary.total_download_size)
        ))
    } else {
        None
    };

    let net = summary.total_installed_size - summary.total_removed_size;
    let net_text = if net != 0 {
        let sign = if net > 0 { "+" } else { "-" };
        Some(format!(
            "Net {sign}{}",
            pakajo::utils::format_bytes(net.abs())
        ))
    } else {
        None
    };

    let mut card = Column::new().spacing(10);

    let mut top = Row::new()
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(8);
    top = top.push(text(counts_left).font(cosmic::font::semibold()));
    top = top.push(space::horizontal());
    if let Some(download) = download_right {
        top = top.push(muted(download));
    }
    card = card.push(top);

    if let Some(net_label) = net_text {
        card = card.push(muted(net_label));
    }

    card = card.push(divider());

    let total = summary.packages.len();
    for pkg in summary.packages.iter().take(6) {
        card = card.push(summary_row(pkg));
    }

    let extra = total.saturating_sub(6);
    if extra > 0 {
        card = card.push(muted(format!("+{extra} more")));
    }

    card.into()
}

fn blocked_banner(failure: &PrepareFailure) -> cosmic::Element<'static, crate::Message> {
    let mut col = Column::new().spacing(6);
    col = col.push(text("Cannot complete this upgrade").font(cosmic::font::semibold()));
    match failure {
        PrepareFailure::Unsatisfied(deps) => {
            for dep in deps {
                col = col.push(muted(format!("{} required by {}", dep.depend, dep.target)));
            }
        }
        PrepareFailure::Other(message) => {
            col = col.push(muted(message.clone()));
        }
    }
    container(col)
        .padding(12)
        .style(|theme: &cosmic::Theme| {
            let warn = Color::from(theme.cosmic().warning.base);
            container::Style {
                text_color: Some(warn),
                background: Some(cosmic::iced::Background::Color(Color { a: 0.12, ..warn })),
                border: cosmic::iced::Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: warn,
                },
                ..Default::default()
            }
        })
        .into()
}
