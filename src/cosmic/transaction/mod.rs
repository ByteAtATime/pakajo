use std::env::current_exe;

use cosmic::app::Task;
use cosmic::widget::{Column, scrollable, text};
use futures::{SinkExt as _, StreamExt as _, channel::oneshot};

use pakajo::dry_run::{dry_run_for_repo_targets, dry_run_for_target};
use pakajo::events::InstallEvent;
use pakajo::install::{ChildOutcome, StreamItem, run_install_process};
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::question::{QuestionSet, collect_approvals, encode_approvals};
use pakajo::transaction_state::InstallKind;

mod state;

pub(crate) use state::{TransactionModel, TransactionStatus};

mod accordion;
use accordion::{action_footer, stage_row};

mod pkgbuild;
use pkgbuild::{PkgbuildMessage, PkgbuildModel};

mod review;
use review::{ReviewMessage, ReviewModel};

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
    Review(ReviewMessage),
    PkgbuildResult(Result<Vec<PkgbuildDiff>, String>),
    ApprovePkgbuild,
    CancelPkgbuild,
    Pkgbuild(PkgbuildMessage),
    Close,
}

pub(crate) enum Action {
    None,
    Run(Task<crate::Message>),
    Finished,
    InstallSucceeded,
}

pub(crate) struct Transaction {
    model: TransactionModel,
}

impl Transaction {
    pub(crate) fn start(name: String, source: PackageSource) -> (Self, Task<crate::Message>) {
        let model = TransactionModel::new(name.clone(), source, InstallKind::Install);
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
        let task = Task::perform(
            async move {
                match orx.await {
                    Ok(Ok(qs)) => Ok(qs),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("dry-run channel closed".to_string()),
                }
            },
            |result| crate::Message::Transaction(TransactionMessage::DryRunResult(result)).into(),
        );
        (Self { model }, task)
    }

    pub(crate) fn update(&mut self, message: TransactionMessage) -> Action {
        match message {
            TransactionMessage::StartInstall => Action::None,
            TransactionMessage::StartRemove => {
                eprintln!("[pakajo] transaction: remove");
                Action::None
            }
            TransactionMessage::InstallEvent(ev) => {
                self.model.apply_event(&ev);
                Action::None
            }
            TransactionMessage::InstallDone(outcome) => {
                eprintln!("[pakajo] install outcome: {outcome:?}");
                let succeeded = matches!(outcome, ChildOutcome::Success);
                self.model.finish(outcome);
                if succeeded {
                    Action::InstallSucceeded
                } else {
                    Action::None
                }
            }
            TransactionMessage::DryRunResult(result) => match result {
                Err(e) => {
                    eprintln!("[pakajo] dry-run failed, proceeding with install: {e}");
                    self.launch_subprocess(None)
                }
                Ok(qs) => {
                    let needs_review = !qs.conflicts.is_empty()
                        || !qs.providers.is_empty()
                        || qs.had_unsupported_question;
                    if !needs_review {
                        return self.proceed_after_conflicts(None);
                    }
                    eprintln!(
                        "[pakajo] review required ({} conflicts, {} providers)",
                        qs.conflicts.len(),
                        qs.providers.len()
                    );
                    let review = ReviewModel::new(qs);
                    self.model.review = Some(review);
                    Action::None
                }
            },
            TransactionMessage::ToggleStage(i) => {
                self.model.toggle(i);
                Action::None
            }
            TransactionMessage::Review(m) => {
                if let Some(r) = self.model.review.as_mut() {
                    r.update(m);
                }
                Action::None
            }
            TransactionMessage::CancelReview => Action::Finished,
            TransactionMessage::ApproveReview => {
                let mut review = match self.model.review.take() {
                    Some(r) => r,
                    None => return Action::None,
                };
                let approvals = match collect_approvals(
                    &review.qs,
                    &review.conflict_checks,
                    &review.provider_choices,
                )
                .and_then(|approvals| encode_approvals(&approvals))
                {
                    Ok(b64) => Some(b64),
                    Err(e) => {
                        eprintln!("[pakajo] approval encoding failed: {e}");
                        None
                    }
                };
                if matches!(self.model.source, PackageSource::Aur) {
                    review.approving = true;
                    self.model.review = Some(review);
                }
                self.proceed_after_conflicts(approvals)
            }
            TransactionMessage::PkgbuildResult(result) => {
                self.model.review = None;
                match result {
                    Err(e) => {
                        eprintln!("[pakajo] pkgbuild fetch failed, proceeding with install: {e}");
                        let approvals = self.model.pending_approvals.take();
                        self.launch_subprocess(approvals)
                    }
                    Ok(diffs) if diffs.is_empty() => {
                        let approvals = self.model.pending_approvals.take();
                        self.launch_subprocess(approvals)
                    }
                    Ok(diffs) => {
                        self.model.pkgbuild_review = Some(PkgbuildModel::new(diffs));
                        Action::None
                    }
                }
            }
            TransactionMessage::Pkgbuild(m) => {
                if let Some(p) = self.model.pkgbuild_review.as_mut() {
                    p.update(m);
                }
                Action::None
            }
            TransactionMessage::ApprovePkgbuild => {
                if let Some(p) = self.model.pkgbuild_review.take() {
                    for diff in &p.diffs {
                        let _ = mark_seen(&diff.dir);
                    }
                }
                let approvals = self.model.pending_approvals.take();
                self.launch_subprocess(approvals)
            }
            TransactionMessage::CancelPkgbuild => Action::Finished,
            TransactionMessage::Close => Action::Finished,
        }
    }

    fn proceed_after_conflicts(&mut self, approvals: Option<String>) -> Action {
        self.model.pending_approvals = approvals;
        if matches!(self.model.source, PackageSource::Aur) {
            let (otx, orx) = oneshot::channel();
            let target = self.model.name.clone();
            std::thread::spawn(move || {
                let result = prepare_pkgbuild_diffs(std::slice::from_ref(&target));
                let _ = otx.send(result);
            });
            let task = Task::perform(
                async move {
                    match orx.await {
                        Ok(Ok(diffs)) => Ok(diffs),
                        Ok(Err(e)) => Err(format!("{e:#}")),
                        Err(_) => Err("pkgbuild fetch channel closed".to_string()),
                    }
                },
                |result| {
                    crate::Message::Transaction(TransactionMessage::PkgbuildResult(result)).into()
                },
            );
            Action::Run(task)
        } else {
            let approvals = self.model.pending_approvals.take();
            self.launch_subprocess(approvals)
        }
    }

    fn launch_subprocess(&mut self, approvals_b64: Option<String>) -> Action {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("[pakajo] failed to resolve current_exe: {e}");
                return Action::None;
            }
        };
        self.model.status = TransactionStatus::Running;
        let name = self.model.name.clone();
        let (raw_tx, mut raw_rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            run_install_process(exe, vec![name], raw_tx, approvals_b64);
        });
        let stream = Task::stream(cosmic::iced::stream::channel(
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
        ));
        Action::Run(stream)
    }

    pub(crate) fn view(&self) -> cosmic::Element<'_, crate::Message> {
        let title = format!("Installing {}", self.model.name);
        let mut panels = Column::new().spacing(6);
        for (i, stage) in self.model.stages.iter().enumerate() {
            panels = panels.push(stage_row(&self.model, i, *stage));
        }
        let mut col = Column::new().spacing(16).push(text(title)).push(panels);
        if matches!(self.model.status, TransactionStatus::Done(_)) {
            col = col.push(action_footer());
        }
        scrollable(col).into()
    }

    pub(crate) fn dialog(&self) -> Option<cosmic::Element<'_, crate::Message>> {
        if let Some(r) = self.model.review.as_ref() {
            return Some(r.view(&self.model.name));
        }
        self.model.pkgbuild_review.as_ref().map(|p| p.view())
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.model.status,
            TransactionStatus::Checking | TransactionStatus::Running
        )
    }

    pub(crate) fn is_checking(&self) -> bool {
        matches!(self.model.status, TransactionStatus::Checking)
    }

    pub(crate) fn name(&self) -> &str {
        &self.model.name
    }
}
