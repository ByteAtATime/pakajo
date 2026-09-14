use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::container;
use futures::StreamExt as _;

use pakajo::dispatch::exec::{ChildOutcome, StreamItem};
use pakajo::dispatch::operation::{BuildOperation, PrivilegedOperation};
use pakajo::dispatch::protocol::{AutomaticDecider, Completion, classify_completion};
use pakajo::dry_run::{dry_run_for_repo_targets, dry_run_for_target};
use pakajo::events::InstallEvent;
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::progress::{InstallKind, SysupgradePhase};
use pakajo::question::{QuestionSet, collect_approvals, encode_approvals};

use crate::Element;

pub(super) const SYSTEM_AUR_NAME: &str = "system-aur";

mod state;

pub(crate) use state::{TransactionModel, TransactionStatus};

mod stepper;

mod aur;

mod finalize;

mod install;

mod repo;

mod resolve;

mod shared;
pub(crate) use shared::format_signed_bytes;

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
    ViewClosed,
    InstallSucceeded,
    ContinueAur(Vec<String>),
}

fn stream_privileged(operation: PrivilegedOperation) -> Task<crate::Message> {
    stream_items(operation.dispatch(false))
}

fn stream_build(operation: BuildOperation, approvals: Option<String>) -> Task<crate::Message> {
    let decider = Box::new(AutomaticDecider);
    stream_items(operation.dispatch(decider, approvals, false))
}

fn stream_items(mut rx: pakajo::dispatch::exec::DispatchStream) -> Task<crate::Message> {
    Task::stream(channel(
        256,
        move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
            use futures::SinkExt as _;
            while let Some(item) = rx.next().await {
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
        let name_for_dry = name.clone();
        let is_repo = matches!(source, PackageSource::Repo);
        let task = crate::components::task::blocking_task(
            move || {
                if is_repo {
                    dry_run_for_repo_targets(std::slice::from_ref(&name_for_dry))
                } else {
                    dry_run_for_target(&name_for_dry)
                }
            },
            "dry-run channel closed",
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
                let next = classify_completion(&outcome, active_phase, aur_targets);
                match next {
                    Completion::ContinueAur { targets } => Action::ContinueAur(targets),
                    Completion::Completed => {
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
                    Ok(payload) => Some(payload),
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
            TransactionMessage::Close => {
                if self.model.is_sysupgrade() {
                    Action::Finished
                } else {
                    Action::ViewClosed
                }
            }
            TransactionMessage::StartRemove => Action::None,
        }
    }

    fn proceed_after_conflicts(&mut self, approvals: Option<String>) -> Action {
        self.model.pending_approvals = approvals;
        if matches!(self.model.source, PackageSource::Aur) {
            let target = self.model.name.clone();
            let task = crate::components::task::blocking_task(
                move || prepare_pkgbuild_diffs(std::slice::from_ref(&target)),
                "pkgbuild fetch channel closed",
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

    fn launch_subprocess(&mut self, approvals: Option<String>) -> Action {
        if self.model.is_aur() {
            return self.launch_aur_in_process(approvals);
        }
        let sealed = match approvals
            .as_deref()
            .map(|payload| pakajo::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
            .transpose()
        {
            Ok(sealed) => sealed,
            Err(e) => {
                eprintln!("[pakajo] approvals write failed: {e:#}");
                self.model.finish(ChildOutcome::Failed(format!("{e:#}")));
                return Action::None;
            }
        };
        let name = self.model.name.clone();
        self.model.status = TransactionStatus::Running;
        let operation = PrivilegedOperation::Install {
            targets: vec![name],
            as_deps: false,
            approvals: sealed,
        };
        Action::Run(stream_privileged(operation))
    }

    fn launch_aur_in_process(&mut self, approvals: Option<String>) -> Action {
        let name = self.model.name.clone();
        self.model.status = TransactionStatus::Running;
        let operation = BuildOperation {
            targets: vec![name],
            as_deps: false,
        };
        Action::Run(stream_build(operation, approvals))
    }

    fn launch_remove_subprocess(&mut self) -> Action {
        let name = self.model.name.clone();
        self.model.status = TransactionStatus::Running;
        let operation = PrivilegedOperation::Remove {
            targets: vec![name],
        };
        Action::Run(stream_privileged(operation))
    }

    pub(crate) fn start_sysupgrade_repo(
        fingerprint: pakajo::dispatch::approvals::ApprovalsFile,
        approvals: Option<String>,
    ) -> (Self, Task<crate::Message>) {
        let mut transaction = Self {
            model: TransactionModel::new(
                "system".to_string(),
                PackageSource::Repo,
                InstallKind::Upgrade,
            ),
        };
        let sealed = match approvals
            .as_deref()
            .map(|payload| pakajo::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
            .transpose()
        {
            Ok(sealed) => sealed,
            Err(e) => {
                eprintln!("[pakajo] approvals write failed: {e:#}");
                transaction
                    .model
                    .finish(ChildOutcome::Failed(format!("{e:#}")));
                return (transaction, Task::none());
            }
        };
        transaction.model.status = TransactionStatus::Running;
        let operation = PrivilegedOperation::UpgradeRepo {
            no_refresh: false,
            ignores: vec![],
            fingerprint: Some(fingerprint),
            approvals: sealed,
        };
        (transaction, stream_privileged(operation))
    }

    pub(crate) fn start_sysupgrade_aur(targets: Vec<String>) -> (Self, Task<crate::Message>) {
        let mut model = TransactionModel::new(
            SYSTEM_AUR_NAME.to_string(),
            PackageSource::Aur,
            InstallKind::Upgrade,
        );
        model.status = TransactionStatus::Running;
        let operation = BuildOperation {
            targets,
            as_deps: false,
        };
        (Self { model }, stream_build(operation, None))
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

    pub(crate) fn is_sysupgrade(&self) -> bool {
        self.model.is_sysupgrade()
    }

    pub(crate) fn status(&self) -> &TransactionStatus {
        &self.model.status
    }

    pub(crate) fn overall_progress(&self) -> f32 {
        self.model.overall_progress()
    }

    pub(crate) fn kind(&self) -> InstallKind {
        self.model.kind
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
