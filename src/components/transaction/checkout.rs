use cosmic::iced::{Length, alignment::Vertical};
use cosmic::widget::{Column, Row, button, dialog, scrollable, space, text};

use pakajo::events::{SummaryAction, TransactionSummary, classify_action, target_version};
use pakajo::utils::format_bytes;

use super::TransactionMessage;
use super::shared::{
    BadgeColor, accent_color, destructive_color, mono_text, muted, pill, success_color,
    version_change,
};
use crate::Element;

pub(crate) struct CheckoutModel {
    pub(crate) summary: TransactionSummary,
}

impl CheckoutModel {
    pub(crate) fn new(summary: TransactionSummary) -> Self {
        Self { summary }
    }

    pub(crate) fn view(&self, name: &str) -> Element<'_> {
        let mut body = Column::new().spacing(8);
        for pkg in &self.summary.packages {
            body = body.push(package_row(pkg));
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
            .title(format!("Confirm removal of {name}"))
            .control(scrollable(body).height(Length::Fixed(400.0)))
            .primary_action(
                button::destructive("Remove").on_press(crate::Message::Transaction(
                    TransactionMessage::ApproveCheckout,
                )),
            )
            .secondary_action(
                button::standard("Cancel").on_press(crate::Message::Transaction(
                    TransactionMessage::CancelCheckout,
                )),
            )
            .into()
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
    fn empty_part1_starts_transaction() {
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
        assert!(tx.model.checkout.is_none());
        assert!(matches!(tx.model.status, TransactionStatus::Running));
    }

    #[test]
    fn converged_install_launches_with_sealed_payload() {
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
        assert!(tx.model.checkout.is_none());
        assert!(matches!(tx.model.status, TransactionStatus::Running));
    }

    #[test]
    fn checkout_cancel_does_not_launch() {
        let mut tx = install_model();
        tx.model.checkout = Some(CheckoutModel::new(summary()));
        let action = tx.update(TransactionMessage::CancelCheckout);
        assert!(matches!(action, Action::Finished));
        assert!(!matches!(tx.model.status, TransactionStatus::Running));
    }
}
