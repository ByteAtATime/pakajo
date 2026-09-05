use std::env::current_exe;

use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::container;
use futures::{SinkExt as _, StreamExt as _, channel::oneshot};

use pakajo::build::{BuildDecision, run_build};
use pakajo::dry_run::{dry_run_for_repo_targets, dry_run_for_target};
use pakajo::events::InstallEvent;
use pakajo::install::{ChildOutcome, StreamItem, run_install_process};
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::progress::{InstallKind, SysupgradePhase};
use pakajo::question::{QuestionSet, collect_approvals, encode_approvals};
use pakajo::remove::run_remove_process;
use pakajo::upgrade::run_sysupgrade_process;

use crate::Element;

pub(super) const SYSTEM_AUR_NAME: &str = "system-aur";

mod state;

pub(crate) use state::{TransactionModel, TransactionStatus};

mod accordion;

mod aur;

mod download;

mod finalize;

mod install;

mod repo;

mod resolve;

mod shared;
pub(crate) use shared::format_signed_bytes;

mod validate;

mod pkgbuild;
pub(crate) use pkgbuild::diff_lines_column;
use pkgbuild::{PkgbuildMessage, PkgbuildModel};

pub(crate) mod review;
use review::{ReviewMessage, ReviewModel};

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartRemove,
    InstallEvent(InstallEvent),
    InstallDone(ChildOutcome),
    DryRunResult(Result<QuestionSet, String>),
    ToggleStage(usize),
    ToggleBuildCard(String),
    Tick(std::time::Instant),
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
    ContinueAur(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum NextInstallState {
    ContinueAur { targets: Vec<String> },
    Completed,
    Cancelled,
    Failed { message: String },
}

fn classify_outcome(
    outcome: &ChildOutcome,
    active_phase: Option<SysupgradePhase>,
    aur_targets: &[String],
) -> NextInstallState {
    if matches!(outcome, ChildOutcome::Success)
        && active_phase == Some(SysupgradePhase::Repo)
        && !aur_targets.is_empty()
    {
        return NextInstallState::ContinueAur {
            targets: aur_targets.to_vec(),
        };
    }
    match outcome {
        ChildOutcome::Success => NextInstallState::Completed,
        ChildOutcome::Dismissed => NextInstallState::Cancelled,
        ChildOutcome::NotFound => NextInstallState::Failed {
            message: "install child not found".to_string(),
        },
        ChildOutcome::Failed(message) => NextInstallState::Failed {
            message: message.clone(),
        },
    }
}

struct CosmicBuildSink {
    tx: futures::channel::mpsc::Sender<StreamItem>,
}

impl pakajo::events::InstallSink for CosmicBuildSink {
    fn event(&mut self, event: InstallEvent) {
        let _ = self.tx.try_send(StreamItem::Event(event));
    }
}

fn spawn_transaction_stream(
    worker: impl FnOnce(futures::channel::mpsc::Sender<StreamItem>) + Send + 'static,
) -> Task<crate::Message> {
    let (raw_tx, mut raw_rx) = futures::channel::mpsc::channel::<StreamItem>(256);
    std::thread::spawn(move || {
        worker(raw_tx);
    });
    Task::stream(channel(
        256,
        move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
            while let Some(item) = raw_rx.next().await {
                match item {
                    StreamItem::Event(ev) => {
                        let _ = tx
                            .send(
                                crate::Message::Transaction(TransactionMessage::InstallEvent(ev))
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

    pub(crate) fn start_remove(
        name: String,
        source: PackageSource,
    ) -> (Self, Task<crate::Message>) {
        let mut transaction = Transaction {
            model: TransactionModel::new(name, source, InstallKind::Remove),
        };
        eprintln!("[pakajo] transaction: remove started");
        let action = transaction.launch_remove_subprocess();
        let task = match action {
            Action::Run(task) => task,
            _ => Task::none(),
        };
        (transaction, task)
    }

    pub(crate) fn update(
        &mut self,
        message: TransactionMessage,
        active_phase: Option<SysupgradePhase>,
        aur_targets: &[String],
    ) -> Action {
        match message {
            TransactionMessage::StartInstall => Action::None,
            TransactionMessage::InstallEvent(ev) => {
                self.model.apply_event(&ev);
                Action::None
            }
            TransactionMessage::InstallDone(outcome) => {
                eprintln!("[pakajo] install outcome: {outcome:?}");
                let next = classify_outcome(&outcome, active_phase, aur_targets);
                match next {
                    NextInstallState::ContinueAur { targets } => Action::ContinueAur(targets),
                    NextInstallState::Completed => {
                        self.model.finish(outcome);
                        Action::InstallSucceeded
                    }
                    _ => {
                        self.model.finish(outcome);
                        Action::None
                    }
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
            TransactionMessage::ToggleBuildCard(name) => {
                self.model.toggle_build_card(name);
                Action::None
            }
            TransactionMessage::Tick(now) => {
                self.model.tick(now);
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
            TransactionMessage::StartRemove => Action::None,
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
        let stream = spawn_transaction_stream(move |tx| {
            run_install_process(exe, vec![name], tx, approvals_b64);
        });
        Action::Run(stream)
    }

    fn launch_remove_subprocess(&mut self) -> Action {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("[pakajo] failed to resolve current_exe: {e}");
                return Action::None;
            }
        };
        self.model.status = TransactionStatus::Running;
        let name = self.model.name.clone();
        let stream = spawn_transaction_stream(move |tx| {
            run_remove_process(exe, vec![name], tx);
        });
        Action::Run(stream)
    }

    pub(crate) fn start_sysupgrade_repo(
        fingerprint_file: String,
        approvals_b64: Option<String>,
    ) -> (Self, Task<crate::Message>) {
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("[pakajo] failed to resolve current_exe: {e}");
                return (
                    Self {
                        model: TransactionModel::new(
                            "system".to_string(),
                            PackageSource::Repo,
                            InstallKind::Upgrade,
                        ),
                    },
                    Task::none(),
                );
            }
        };
        let mut model = TransactionModel::new(
            "system".to_string(),
            PackageSource::Repo,
            InstallKind::Upgrade,
        );
        model.status = TransactionStatus::Running;
        let stream = spawn_transaction_stream(move |tx| {
            run_sysupgrade_process(exe, fingerprint_file, tx, approvals_b64);
        });
        (Self { model }, stream)
    }

    pub(crate) fn start_sysupgrade_aur(targets: Vec<String>) -> (Self, Task<crate::Message>) {
        let mut model = TransactionModel::new(
            SYSTEM_AUR_NAME.to_string(),
            PackageSource::Aur,
            InstallKind::Upgrade,
        );
        model.status = TransactionStatus::Running;
        let stream = spawn_transaction_stream(move |mut raw_tx| {
            let mut sink = CosmicBuildSink { tx: raw_tx.clone() };
            let result = run_build(
                &targets,
                false,
                false,
                &mut sink,
                |_| BuildDecision::Proceed,
                |_| true,
                None,
            );
            let outcome = match result {
                Ok(()) => ChildOutcome::Success,
                Err(e) => ChildOutcome::Failed(format!("{e:#}")),
            };
            let _ = raw_tx.try_send(StreamItem::Done(outcome));
        });
        (Self { model }, stream)
    }

    pub(crate) fn view(&self) -> Element<'_> {
        if self.model.is_aur() {
            return aur::view(&self.model);
        }
        repo::view(&self.model)
    }

    pub(crate) fn dialog(&self) -> Option<Element<'_>> {
        let content = if let Some(r) = self.model.review.as_ref() {
            r.view(&self.model.name)
        } else {
            self.model.pkgbuild_review.as_ref().map(|p| p.view())?
        };
        Some(dialog_backdrop(content))
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.model.status,
            TransactionStatus::Checking | TransactionStatus::Running
        )
    }

    pub(crate) fn building(&self) -> bool {
        self.model.building()
    }

    pub(crate) fn is_checking(&self) -> bool {
        matches!(self.model.status, TransactionStatus::Checking)
    }

    pub(crate) fn name(&self) -> &str {
        &self.model.name
    }
}

fn dialog_backdrop(content: Element<'_>) -> Element<'_> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_outcome_routes_child_results() {
        let aur_targets = vec!["aur-pkg".to_string()];
        let cases = [
            (
                ChildOutcome::Success,
                Some(SysupgradePhase::Repo),
                aur_targets.clone(),
                NextInstallState::ContinueAur {
                    targets: aur_targets.clone(),
                },
            ),
            (
                ChildOutcome::Success,
                Some(SysupgradePhase::Repo),
                Vec::new(),
                NextInstallState::Completed,
            ),
            (
                ChildOutcome::Success,
                None,
                Vec::new(),
                NextInstallState::Completed,
            ),
            (
                ChildOutcome::Dismissed,
                None,
                Vec::new(),
                NextInstallState::Cancelled,
            ),
            (
                ChildOutcome::NotFound,
                None,
                Vec::new(),
                NextInstallState::Failed {
                    message: "install child not found".to_string(),
                },
            ),
            (
                ChildOutcome::Failed("err".to_string()),
                None,
                Vec::new(),
                NextInstallState::Failed {
                    message: "err".to_string(),
                },
            ),
        ];
        for (outcome, phase, targets, expected) in cases {
            assert_eq!(
                classify_outcome(&outcome, phase, &targets),
                expected,
                "outcome {outcome:?} phase {phase:?} targets {targets:?}"
            );
        }
    }
}
