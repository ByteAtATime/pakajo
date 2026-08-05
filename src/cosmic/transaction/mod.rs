use std::collections::HashMap;
use std::env::current_exe;

use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget::{Column, Row, button, checkbox, container, radio, scrollable, space, text};
use futures::{SinkExt as _, StreamExt as _, channel::oneshot};

use pakajo::dry_run::{dry_run_for_repo_targets, dry_run_for_target};
use pakajo::events::InstallEvent;
use pakajo::install::{ChildOutcome, StreamItem, run_install_process};
use pakajo::package::PackageSource;
use pakajo::question::{ProviderCandidate, QuestionSet, collect_approvals, encode_approvals};
use pakajo::transaction_state::InstallKind;

use crate::detail::DetailData;

mod state;

pub(crate) use state::{TransactionModel, TransactionStatus};

mod accordion;
use accordion::{action_footer, stage_row};

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartRemove,
    InstallEvent(InstallEvent),
    InstallDone(ChildOutcome),
    DryRunResult(Result<QuestionSet, String>),
    ToggleStage(usize),
    ApproveReview,
    CancelReview,
    ToggleConflict(usize),
    SelectProvider { depend: String, idx: usize },
    Close,
}

struct ConflictReview {
    qs: QuestionSet,
    conflict_checks: Vec<bool>,
    provider_choices: HashMap<String, usize>,
}

impl crate::PakajoApp {
    pub(crate) fn handle_transaction(
        &mut self,
        message: TransactionMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            TransactionMessage::StartInstall => self.start_install(),
            TransactionMessage::StartRemove => {
                eprintln!("[pakajo] transaction: remove");
                Task::none()
            }
            TransactionMessage::InstallEvent(ev) => {
                if let Some(model) = self.transaction.as_mut() {
                    model.apply_event(&ev);
                }
                Task::none()
            }
            TransactionMessage::InstallDone(outcome) => {
                self.transacting = false;
                eprintln!("[pakajo] install outcome: {outcome:?}");
                if matches!(outcome, ChildOutcome::Success) {
                    // TODO: refresh updates
                }
                if let Some(model) = self.transaction.as_mut() {
                    model.finish(outcome);
                }
                Task::none()
            }
            TransactionMessage::DryRunResult(result) => match result {
                Err(e) => {
                    eprintln!("[pakajo] dry-run failed, proceeding with install: {e}");
                    return self.launch_install_subprocess(None);
                }
                Ok(qs) => {
                    let needs_review = !qs.conflicts.is_empty()
                        || !qs.providers.is_empty()
                        || qs.had_unsupported_question;
                    if !needs_review {
                        return self.launch_install_subprocess(None);
                    }
                    eprintln!(
                        "[pakajo] review required ({} conflicts, {} providers)",
                        qs.conflicts.len(),
                        qs.providers.len()
                    );
                    let conflict_count = qs.conflicts.len();
                    let provider_choices = qs
                        .providers
                        .iter()
                        .map(|prompt| (prompt.depend.clone(), 0))
                        .collect();
                    let review = ConflictReview {
                        qs,
                        conflict_checks: vec![true; conflict_count],
                        provider_choices,
                    };
                    if let Some(model) = self.transaction.as_mut() {
                        model.review = Some(review);
                    }
                    Task::none()
                }
            },
            TransactionMessage::ToggleStage(i) => {
                if let Some(model) = self.transaction.as_mut() {
                    model.toggle(i);
                }
                Task::none()
            }
            TransactionMessage::ToggleConflict(i) => {
                if let Some(check) = self
                    .transaction
                    .as_mut()
                    .and_then(|m| m.review.as_mut())
                    .and_then(|r| r.conflict_checks.get_mut(i))
                {
                    *check = !*check;
                }
                Task::none()
            }
            TransactionMessage::SelectProvider { depend, idx } => {
                if let Some(choices) = self
                    .transaction
                    .as_mut()
                    .and_then(|m| m.review.as_mut())
                    .map(|r| &mut r.provider_choices)
                {
                    choices.insert(depend, idx);
                }
                Task::none()
            }
            TransactionMessage::CancelReview => {
                self.transaction = None;
                self.transacting = false;
                Task::none()
            }
            TransactionMessage::ApproveReview => {
                let review = match self.transaction.as_mut().and_then(|m| m.review.take()) {
                    Some(r) => r,
                    None => return Task::none(),
                };
                match collect_approvals(
                    &review.qs,
                    &review.conflict_checks,
                    &review.provider_choices,
                )
                .and_then(|approvals| encode_approvals(&approvals))
                {
                    Ok(b64) => self.launch_install_subprocess(Some(b64)),
                    Err(e) => {
                        eprintln!("[pakajo] approval encoding failed: {e}");
                        self.launch_install_subprocess(None)
                    }
                }
            }
            TransactionMessage::Close => {
                self.transaction = None;
                Task::none()
            }
        }
    }

    fn start_install(&mut self) -> cosmic::app::Task<crate::Message> {
        if self.transacting {
            return Task::none();
        }
        let (name, source) = match &self.detail {
            DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source),
            _ => return Task::none(),
        };
        self.transaction = Some(TransactionModel::new(name.clone(), InstallKind::Install));
        self.transacting = true;
        let (otx, orx) = oneshot::channel();
        let name_for_dry = name.clone();
        let is_repo = matches!(source, PackageSource::Repo);
        std::thread::spawn(move || {
            let result = if is_repo {
                dry_run_for_repo_targets(std::slice::from_ref(&name_for_dry))
            } else {
                dry_run_for_target(&name_for_dry)
            };
            let _ = otx.send(result);
        });
        Task::perform(
            async move {
                match orx.await {
                    Ok(Ok(qs)) => Ok(qs),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("dry-run channel closed".to_string()),
                }
            },
            |result| crate::Message::Transaction(TransactionMessage::DryRunResult(result)).into(),
        )
    }

    fn launch_install_subprocess(
        &mut self,
        approvals_b64: Option<String>,
    ) -> cosmic::app::Task<crate::Message> {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("[pakajo] failed to resolve current_exe: {e}");
                return Task::none();
            }
        };
        let model = match self.transaction.as_mut() {
            Some(model) => model,
            None => return Task::none(),
        };
        model.status = TransactionStatus::Running;
        let name = model.name.clone();
        let (raw_tx, mut raw_rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            run_install_process(exe, vec![name], raw_tx, approvals_b64);
        });
        Task::stream(cosmic::iced::stream::channel(
            256,
            move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
                while let Some(item) = raw_rx.next().await {
                    match item {
                        StreamItem::Event(ev) => {
                            let _ = tx
                                .send(
                                    crate::Message::Transaction(TransactionMessage::InstallEvent(
                                        ev,
                                    ))
                                    .into(),
                                )
                                .await;
                        }
                        StreamItem::Done(outcome) => {
                            let _ = tx
                                .send(
                                    crate::Message::Transaction(TransactionMessage::InstallDone(
                                        outcome,
                                    ))
                                    .into(),
                                )
                                .await;
                            break;
                        }
                    }
                }
            },
        ))
    }
}

pub(crate) fn transaction_view(model: &TransactionModel) -> cosmic::Element<'_, crate::Message> {
    if let Some(review) = &model.review {
        return review_view(model, review);
    }
    if matches!(model.status, TransactionStatus::Checking) {
        let col = Column::new().push(text("Checking for conflicts…"));
        return scrollable(col).into();
    }
    let title = format!("Installing {}", model.name);
    let mut panels = Column::new().spacing(6);
    for (i, stage) in model.stages.iter().enumerate() {
        panels = panels.push(stage_row(model, i, *stage));
    }
    let mut col = Column::new().spacing(16).push(text(title)).push(panels);
    if matches!(model.status, TransactionStatus::Done(_)) {
        col = col.push(action_footer());
    }
    scrollable(col).into()
}

fn review_view<'a>(
    model: &'a TransactionModel,
    review: &'a ConflictReview,
) -> cosmic::Element<'a, crate::Message> {
    let title = format!("Review installation of {}", model.name);
    let mut col = Column::new().spacing(16).push(text(title));

    if !review.qs.conflicts.is_empty() {
        col = col.push(text("Conflicts"));
        for (i, conflict) in review.qs.conflicts.iter().enumerate() {
            let label = format!("Replace {} with {}", conflict.removable, conflict.incoming);
            let checked = review.conflict_checks.get(i).copied().unwrap_or(false);
            let item = checkbox(checked).label(label).on_toggle(move |_| {
                crate::Message::Transaction(TransactionMessage::ToggleConflict(i))
            });
            col = col.push(item);
        }
    }

    if !review.qs.providers.is_empty() {
        col = col.push(text("Providers"));
        for prompt in &review.qs.providers {
            col = col.push(text(prompt.depend.clone()));
            let selected = review
                .provider_choices
                .get(&prompt.depend)
                .copied()
                .unwrap_or(0);
            for (idx, candidate) in prompt.candidates.iter().enumerate() {
                let label = candidate_label(candidate);
                let depend = prompt.depend.clone();
                let item = radio(text(label), idx, Some(selected), move |chosen: usize| {
                    crate::Message::Transaction(TransactionMessage::SelectProvider {
                        depend: depend.clone(),
                        idx: chosen,
                    })
                });
                col = col.push(item);
            }
        }
    }

    if review.qs.had_unsupported_question {
        col = col.push(unsupported_banner(&review.qs.unsupported_summary));
    }

    col = col.push(review_footer());
    scrollable(col).into()
}

fn candidate_label(candidate: &ProviderCandidate) -> String {
    let qualified = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    match &candidate.version {
        Some(version) => format!("{qualified}  {version}"),
        None => qualified,
    }
}

fn unsupported_banner(summary: &str) -> cosmic::Element<'static, crate::Message> {
    container(text(summary.to_string()))
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            text_color: Some(Color::from(theme.cosmic().warning.on)),
            background: Some(Background::Color(Color::from(theme.cosmic().warning.base))),
            border: cosmic::iced::Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color::from(theme.cosmic().warning.base),
            },
            ..Default::default()
        })
        .into()
}

fn review_footer() -> cosmic::Element<'static, crate::Message> {
    let cancel = button::custom(text("Cancel")).on_press(crate::Message::Transaction(
        TransactionMessage::CancelReview,
    ));
    let confirm = button::custom(text("Confirm")).on_press(crate::Message::Transaction(
        TransactionMessage::ApproveReview,
    ));
    Row::new()
        .spacing(8)
        .push(space::horizontal())
        .push(cancel)
        .push(confirm)
        .into()
}
