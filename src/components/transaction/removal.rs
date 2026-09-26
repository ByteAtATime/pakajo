use cosmic::iced::alignment::Vertical;
use cosmic::widget::{Column, Row, button, dialog, space, text};

use pakajo::events::{SummaryPackage, TransactionSummary};
use pakajo::utils::format_bytes;

use super::TransactionMessage;
use super::shared::{accent_color, mono_text, muted, pill};
use crate::Element;

pub(crate) struct RemovalConfirmModel {
    pub(crate) package: SummaryPackage,
    pub(crate) freed_size: i64,
    description: Option<String>,
    repo: Option<String>,
}

impl RemovalConfirmModel {
    pub(crate) fn new(
        summary: TransactionSummary,
        description: Option<String>,
        repo: Option<String>,
    ) -> Option<Self> {
        if summary.packages.len() != 1 {
            eprintln!(
                "[pakajo] removal confirmation requires exactly one package, got {}",
                summary.packages.len()
            );
            return None;
        }
        let freed_size = summary.total_removed_size;
        summary.packages.into_iter().next().map(|package| Self {
            repo: package
                .repository
                .clone()
                .or(repo)
                .filter(|repo| !repo.is_empty()),
            description: description.filter(|d| !d.is_empty()),
            package,
            freed_size,
        })
    }

    pub(crate) fn view(&self) -> Element<'_> {
        let pkg = &self.package;
        let header = Row::new()
            .align_y(Vertical::Center)
            .spacing(8)
            .push(mono_text(&pkg.name))
            .push_maybe(
                pkg.old_version
                    .as_deref()
                    .map(|v| muted(text::monotext(v.to_string()))),
            )
            .push_maybe(
                self.repo
                    .as_ref()
                    .map(|repo| pill(repo.clone(), accent_color)),
            )
            .push(space::horizontal())
            .push_maybe((self.freed_size > 0).then(|| {
                muted(text::monotext(format!(
                    "-{}",
                    format_bytes(self.freed_size)
                )))
            }));
        let body = Column::new().spacing(8).push(header).push_maybe(
            self.description
                .as_ref()
                .map(|description| muted(text(description.clone()).size(13))),
        );
        dialog()
            .title(format!("Remove {}?", pkg.name))
            .control(body)
            .primary_action(
                button::destructive("Remove").on_press(crate::Message::Transaction(
                    TransactionMessage::ApproveRemoval,
                )),
            )
            .secondary_action(
                button::standard("Cancel").on_press(crate::Message::Transaction(
                    TransactionMessage::CancelRemoval,
                )),
            )
            .into()
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use crate::components::transaction::state::{TransactionModel, TransactionStatus};
    use crate::components::transaction::{Action, Transaction};
    use pakajo::progress::InstallKind;

    fn removal_summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![SummaryPackage {
                name: "firefox".to_string(),
                repository: None,
                new_version: String::new(),
                old_version: Some("1.0".to_string()),
                download_size: 0,
                installed_size: 0,
                old_installed_size: 2,
                is_removal: true,
            }],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 2,
        }
    }

    fn removal_transaction() -> Transaction {
        Transaction {
            model: TransactionModel::batch(
                "firefox".to_string(),
                vec!["firefox".to_string()],
                vec![],
                InstallKind::Remove,
            ),
            review_loop: None,
        }
    }

    #[test]
    fn removal_confirm_rejects_multiple_packages() {
        let mut summary = removal_summary();
        let duplicate = summary.packages[0].clone();
        summary.packages.push(duplicate);
        assert!(RemovalConfirmModel::new(summary, None, None).is_none());
    }

    #[test]
    fn removal_cancel_does_not_launch() {
        let mut tx = removal_transaction();
        tx.model.removal_confirm =
            Some(RemovalConfirmModel::new(removal_summary(), None, None).expect("single package"));
        let action = tx.update(TransactionMessage::CancelRemoval);
        assert!(matches!(action, Action::Finished));
        assert!(!matches!(tx.model.status, TransactionStatus::Running));
    }
}
