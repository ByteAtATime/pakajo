use cosmic::iced::{Length, alignment::Vertical};
use cosmic::widget::{Column, Row, button, dialog, scrollable, space, text};

use pakajo::events::{SummaryAction, TransactionSummary, classify_action, target_version};
use pakajo::progress::InstallKind;
use pakajo::upgrade::AurUpgradeCandidate;
use pakajo::utils::format_bytes;

use super::TransactionMessage;
use super::shared::{
    BadgeColor, accent_color, destructive_color, mono_text, muted, pill, success_color,
    version_change,
};
use crate::Element;
use crate::components::updates::aur_upgrade_row;

pub(crate) struct CheckoutModel {
    pub(crate) summary: TransactionSummary,
    kind: InstallKind,
    aur: Vec<AurUpgradeCandidate>,
}

impl CheckoutModel {
    pub(crate) fn new(
        summary: TransactionSummary,
        kind: InstallKind,
        aur: Vec<AurUpgradeCandidate>,
    ) -> Self {
        Self { summary, kind, aur }
    }

    pub(crate) fn view(&self, name: &str) -> Element<'_> {
        let mut body = Column::new().spacing(8);
        for pkg in &self.summary.packages {
            body = body.push(package_row(pkg));
        }
        if self.kind == InstallKind::Upgrade && !self.aur.is_empty() {
            let mut aur_col = Column::new().spacing(8);
            aur_col = aur_col.push(text(format!("AUR packages to build ({})", self.aur.len())));
            for candidate in &self.aur {
                aur_col = aur_col.push(aur_upgrade_row(candidate));
            }
            body = body.push(aur_col);
        }
        body = body.push(text("Totals"));
        body = body.push(muted(text(format!(
            "Download: {}",
            format_bytes(self.summary.total_download_size)
        ))));
        body = body.push(muted(text(format!(
            "Installed: +{}",
            format_bytes(self.summary.total_installed_size)
        ))));
        body = body.push(muted(text(format!(
            "Removed: {}",
            format_bytes(self.summary.total_removed_size)
        ))));
        dialog()
            .title(checkout_title(name, self.kind))
            .control(scrollable(body).height(Length::Fixed(400.0)))
            .primary_action(checkout_confirm(self.kind))
            .secondary_action(
                button::standard("Cancel").on_press(crate::Message::Transaction(
                    TransactionMessage::CancelCheckout,
                )),
            )
            .into()
    }
}

fn checkout_title(name: &str, kind: InstallKind) -> String {
    match kind {
        InstallKind::Remove => format!("Confirm removal of {name}"),
        InstallKind::Install | InstallKind::Upgrade => {
            format!("Confirm installation of {name}")
        }
    }
}

fn checkout_confirm_label(kind: InstallKind) -> &'static str {
    match kind {
        InstallKind::Remove => "Remove",
        InstallKind::Install | InstallKind::Upgrade => "Proceed",
    }
}

fn checkout_confirm(kind: InstallKind) -> Element<'static> {
    let pressed = crate::Message::Transaction(TransactionMessage::ApproveCheckout);
    match kind {
        InstallKind::Remove => button::destructive(checkout_confirm_label(kind))
            .on_press(pressed)
            .into(),
        InstallKind::Install | InstallKind::Upgrade => {
            button::suggested(checkout_confirm_label(kind))
                .on_press(pressed)
                .into()
        }
    }
}

fn action_color(action: SummaryAction) -> BadgeColor {
    match action {
        SummaryAction::Install | SummaryAction::Reinstall => success_color,
        SummaryAction::Upgrade | SummaryAction::Downgrade => accent_color,
        SummaryAction::Remove => destructive_color,
    }
}

fn package_row(pkg: &pakajo::events::SummaryPackage) -> Element<'_> {
    let action = classify_action(pkg);
    let mut row = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(mono_text(&pkg.name))
        .push(pill(format!("{action:?}"), action_color(action)));
    if let Some(repo) = &pkg.repository {
        row = row.push(muted(text(repo.clone())));
    }
    row = row
        .push(space::horizontal())
        .push(muted(text::monotext(version_change(
            pkg.old_version.as_deref(),
            Some(target_version(pkg)),
        ))));
    row.into()
}

#[cfg(test)]
mod checkout_tests {
    use super::*;
    use crate::components::transaction::review::InstallReview;
    use crate::components::transaction::state::{TransactionModel, TransactionStatus};
    use crate::components::transaction::{Action, Transaction};
    use pakajo::events::SummaryPackage;
    use pakajo::progress::InstallKind;
    use pakajo::question::model::Question;

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![SummaryPackage {
                name: s("firefox"),
                repository: Some(s("extra")),
                new_version: s("1.0"),
                old_version: None,
                download_size: 1,
                installed_size: 2,
                old_installed_size: 0,
                is_removal: false,
            }],
            total_download_size: 1,
            total_installed_size: 2,
            total_removed_size: 0,
        }
    }

    fn install_model() -> Transaction {
        let model = TransactionModel::batch(
            s("firefox"),
            vec![s("firefox")],
            vec![],
            false,
            InstallKind::Install,
        );
        Transaction {
            model,
            review_loop: None,
        }
    }

    fn conflict() -> Question {
        Question::Conflict {
            incoming: s("cava-git"),
            incoming_version: s("1.0-1"),
            removable: s("cava"),
            removable_version: s("1.0-1"),
            conflict_reason: None,
        }
    }

    fn conflict_answer() -> pakajo::question::model::Answer {
        pakajo::question::model::Answer::Conflict {
            incoming: s("cava-git"),
            removable: s("cava"),
            remove: true,
        }
    }

    fn revalidated() -> pakajo::dispatch::RevalidationRun {
        pakajo::dispatch::RevalidationRun {
            origin: pakajo::dispatch::ReviewOrigin::Revalidation,
            questions: vec![conflict()],
            answers: vec![conflict_answer()],
            summary: summary(),
            aur: Vec::new(),
        }
    }

    #[test]
    fn empty_part1_goes_straight_to_checkout() {
        let mut tx = install_model();
        tx.update(TransactionMessage::Explored(Ok(
            pakajo::dispatch::RevalidationRun {
                origin: pakajo::dispatch::ReviewOrigin::Initial,
                questions: vec![],
                answers: Vec::new(),
                summary: summary(),
                aur: Vec::new(),
            },
        )));
        assert!(tx.model.install_review.is_none());
        assert_eq!(
            tx.model.checkout.as_ref().expect("checkout shown").summary,
            summary()
        );
    }

    #[test]
    fn checkout_proceed_launches_with_sealed_payload() {
        let mut tx = install_model();
        tx.model.install_review = Some(InstallReview::new(vec![conflict()]));
        tx.model.summary = Some(summary());
        tx.update(TransactionMessage::ApproveReview);
        assert!(tx.model.checkout.is_none());
        assert!(
            tx.model
                .install_review
                .as_ref()
                .expect("review kept")
                .approving
        );
        let sealed = pakajo::dispatch::seal::decode_seal(
            tx.model
                .pending_approvals
                .as_deref()
                .expect("payload sealed"),
        )
        .expect("decodes");
        assert!(sealed.proceed);
        assert_eq!(sealed.answers.len(), 1);
        assert!(matches!(
            sealed.answers[0],
            (
                pakajo::question::model::QuestionKey::Conflict { .. },
                pakajo::question::model::Answer::Conflict { remove: true, .. }
            )
        ));
        tx.update(TransactionMessage::Explored(Ok(revalidated())));
        assert!(tx.model.checkout.is_some());
        tx.update(TransactionMessage::ApproveCheckout);
        assert!(matches!(tx.model.status, TransactionStatus::Running));
    }

    #[test]
    fn checkout_cancel_does_not_launch() {
        let mut tx = install_model();
        tx.model.checkout = Some(CheckoutModel::new(
            summary(),
            InstallKind::Install,
            Vec::new(),
        ));
        let action = tx.update(TransactionMessage::CancelCheckout);
        assert!(matches!(action, Action::Finished));
        assert!(!matches!(tx.model.status, TransactionStatus::Running));
    }
}
