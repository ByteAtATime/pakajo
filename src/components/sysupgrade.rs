use cosmic::app::Task;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{Column, Row, button, container, scrollable, space, text};

use anyhow::Context as _;
use pakajo::dry_run::{PrepareFailure, SysupgradePreview};
use pakajo::events::{SummaryPackage, TransactionSummary};
use pakajo::question::{collect_approvals, default_approve, encode_approvals};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

pub fn next_sysupgrade_step(
    from: crate::Page,
    dir: Direction,
    has_resolve: bool,
    has_diffs: bool,
) -> crate::Page {
    use crate::Page::*;
    use Direction::*;
    match (from, dir) {
        (Updates, Forward) => {
            if has_resolve {
                Resolve
            } else if has_diffs {
                PkgbuildReview
            } else {
                Confirm
            }
        }
        (Resolve, Forward) => {
            if has_diffs {
                PkgbuildReview
            } else {
                Confirm
            }
        }
        (Resolve, Backward) => Updates,
        (PkgbuildReview, Forward) => Confirm,
        (PkgbuildReview, Backward) => {
            if has_resolve {
                Resolve
            } else {
                Updates
            }
        }
        (Confirm, Backward) => {
            if has_diffs {
                PkgbuildReview
            } else if has_resolve {
                Resolve
            } else {
                Updates
            }
        }
        _ => from,
    }
}

use crate::Element;
use crate::components::theme::{
    accent_color, destructive_color, muted_text as muted, pill, success_color, warning_color,
};
use crate::components::transaction::review::{ReviewModel, ReviewSelection, review_body};
use crate::components::transaction::{Transaction, format_signed_bytes};
use crate::components::updates::aur_upgrade_row;
use cosmic::widget::divider;

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

fn no_preview() -> Element<'static> {
    container(text("No preview available"))
        .width(Length::Fill)
        .height(Length::Fill)
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
                pakajo::pacman::refresh_sync_dbs_rootless(&mut handle)?;
                let mut preview =
                    pakajo::dry_run::compute_sysupgrade_preview(&mut handle, &config)?;
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
        let (transaction, task) =
            Transaction::start_sysupgrade_repo(fingerprint_file, approvals_b64);
        self.transaction = Some(transaction);
        self.active_sysupgrade_phase = Some(pakajo::progress::SysupgradePhase::Repo);
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
                            crate::Page::Updates,
                            Direction::Forward,
                            has_resolve,
                            has_diffs,
                        );
                        if matches!(next, crate::Page::PkgbuildReview) {
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
                        self.goto_page(next)
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
        if matches!(self.page, crate::Page::Search | crate::Page::Updates) {
            return Task::none();
        }
        let from = self.page;
        let has_resolve = self.sysupgrade_review.is_some();
        let has_diffs = self
            .sysupgrade_preview
            .as_ref()
            .map(|p| !p.pkgbuild_diffs.is_empty())
            .unwrap_or(false);
        let next = next_sysupgrade_step(from, dir, has_resolve, has_diffs);
        if matches!(next, crate::Page::PkgbuildReview) {
            self.pkgbuild_review_index = 0;
        }
        eprintln!(
            "[pakajo] sysupgrade route {:?} {:?} -> {:?}",
            from, dir, next
        );
        self.goto_page(next)
    }

    pub(crate) fn resolve_page(&self) -> Element<'_> {
        let back =
            button::standard("Back").on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));
        let continue_btn = button::standard("Continue")
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
            let inner =
                review_body(&r.qs, &r.conflict_checks, &r.provider_choices).map(|selection| {
                    crate::Message::Sysupgrade(match selection {
                        ReviewSelection::ToggleConflict(i) => SysupgradeMessage::ToggleConflict(i),
                        ReviewSelection::SelectProvider { depend, idx } => {
                            SysupgradeMessage::SelectProvider { depend, idx }
                        }
                    })
                });
            body = body.push(inner);
        }

        let column = Column::new()
            .push(padded_header)
            .push(scrollable(body).id(crate::page_scroll_id()));
        container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn pkgbuild_review_page(&self) -> Element<'_> {
        let back =
            button::standard("Back").on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));
        let continue_btn = button::standard("Continue")
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
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn confirm_page(&self) -> Element<'_> {
        let back =
            button::standard("Back").on_press(crate::Message::Sysupgrade(SysupgradeMessage::Back));

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
            button::suggested("Apply")
        } else {
            button::suggested("Apply")
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
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        };

        let mut body = Column::new().spacing(16).padding([0.0, 12.0]);

        if let Some(failure) = &preview.prepare_error {
            body = body.push(blocked_banner(failure));
        }

        body = body.push(manifest_section(&preview.summary));

        if !preview.aur.is_empty() {
            let mut aur_col = Column::new().spacing(8);
            aur_col = aur_col.push(text::caption_heading(format!(
                "AUR packages to build ({})",
                preview.aur.len()
            )));
            for candidate in &preview.aur {
                aur_col = aur_col.push(aur_upgrade_row(candidate));
            }
            body = body.push(aur_col);
        }

        let column = Column::new()
            .push(padded_header)
            .push(scrollable(body).id(crate::page_scroll_id()));
        container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

fn summary_row(pkg: &SummaryPackage) -> Element<'_> {
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
        .align_y(Alignment::Center)
        .spacing(8)
        .push(muted(version_text))
        .push(pill(op_label, color_fn));
    Row::new()
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(12)
        .push(left)
        .push(space::horizontal())
        .push(right)
        .into()
}

fn manifest_section(summary: &TransactionSummary) -> Element<'_> {
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
        Some(format!("Net {}", format_signed_bytes(net)))
    } else {
        None
    };

    let mut card = Column::new().spacing(10);

    let mut top = Row::new().align_y(Alignment::Center).spacing(8);
    top = top.push(text::heading(counts_left));
    top = top.push(space::horizontal());
    if let Some(download) = download_right {
        top = top.push(muted(download));
    }
    card = card.push(top);

    if let Some(net_label) = net_text {
        card = card.push(muted(net_label));
    }

    card = card.push(divider::horizontal::default());

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

fn blocked_banner(failure: &PrepareFailure) -> Element<'static> {
    let mut col = Column::new().spacing(6);
    col = col.push(text::heading("Cannot complete this upgrade"));
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
            let warn = warning_color(theme);
            container::Style {
                text_color: Some(warn),
                background: Some(Background::Color(Color { a: 0.12, ..warn })),
                border: Border {
                    radius: theme.cosmic().corner_radii.radius_s.into(),
                    width: 1.0,
                    color: warn,
                },
                ..Default::default()
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Page::*;

    #[test]
    fn next_sysupgrade_step_routes_navigation() {
        use Direction::*;
        let cases = [
            (Updates, Forward, false, false, Confirm),
            (Updates, Forward, false, true, PkgbuildReview),
            (Updates, Forward, true, true, Resolve),
            (Resolve, Forward, true, true, PkgbuildReview),
            (Resolve, Forward, true, false, Confirm),
            (Resolve, Backward, true, true, Updates),
            (PkgbuildReview, Forward, true, true, Confirm),
            (PkgbuildReview, Backward, true, true, Resolve),
            (PkgbuildReview, Backward, false, true, Updates),
            (Confirm, Backward, true, true, PkgbuildReview),
            (Confirm, Backward, true, false, Resolve),
            (Confirm, Backward, false, false, Updates),
            (Confirm, Forward, true, true, Confirm),
            (Updates, Backward, true, true, Updates),
        ];
        for (from, dir, has_resolve, has_diffs, expected) in cases {
            assert_eq!(
                next_sysupgrade_step(from, dir, has_resolve, has_diffs),
                expected,
                "from {from:?} dir {dir:?} has_resolve {has_resolve} has_diffs {has_diffs}"
            );
        }
    }
}
