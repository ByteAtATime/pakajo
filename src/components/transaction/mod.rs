use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::container;
use futures::StreamExt as _;

use pakajo::dispatch::exec::{ChildOutcome, StreamItem};
use pakajo::dispatch::protocol::AutomaticDecider;
use pakajo::events::InstallEvent;
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::progress::InstallKind;
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

pub(crate) mod diff;
mod pkgbuild;
pub(crate) use diff::diff_rows_column;
pub(crate) use pkgbuild::ReviewedDiff;
use pkgbuild::{PkgbuildMessage, PkgbuildModel};

pub(crate) mod review;
use review::{ReviewMessage, ReviewModel};

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartBatchInstall,
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
}

fn review_approvals(review: &ReviewModel) -> Option<String> {
    match collect_approvals(
        &review.qs,
        &review.conflict_checks,
        &review.provider_choices,
        &review.qs.held,
    )
    .and_then(|approvals| encode_approvals(&approvals))
    {
        Ok(payload) => Some(payload),
        Err(e) => {
            eprintln!("[pakajo] approval encoding failed: {e}");
            None
        }
    }
}

fn stream_items(mut rx: pakajo::dispatch::exec::DispatchStream) -> Task<crate::Message> {
    Task::stream(channel(
        256,
        move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
            use futures::SinkExt as _;
            let mut done_seen = false;
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
                        done_seen = true;
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
            if !done_seen {
                let _ = tx
                    .send(
                        crate::Message::Transaction(TransactionMessage::InstallDone(
                            ChildOutcome::Failed("stream ended".into()),
                        ))
                        .into(),
                    )
                    .await;
            }
        },
    ))
}

pub(crate) fn partition_batch_targets(
    targets: &[(String, String)],
    resolve: impl Fn(&str, &str) -> Option<String>,
) -> (Vec<String>, Vec<String>) {
    let mut merged = Vec::with_capacity(targets.len());
    let mut aur = Vec::new();
    for (name, constraint) in targets {
        match resolve(name, constraint) {
            Some(real) => merged.push(real),
            None => {
                merged.push(name.clone());
                aur.push(name.clone());
            }
        }
    }
    (merged, aur)
}

pub(crate) struct Transaction {
    model: TransactionModel,
}

impl Transaction {
    pub(crate) fn start(name: String, source: PackageSource) -> (Self, Task<crate::Message>) {
        let targets = vec![name.clone()];
        let aur_names = match source {
            PackageSource::Aur => vec![name.clone()],
            _ => Vec::new(),
        };
        let prefer_aur = matches!(source, PackageSource::Aur);
        Self::start_batch(targets, aur_names, prefer_aur)
    }

    pub(crate) fn start_batch(
        names: Vec<String>,
        aur_names: Vec<String>,
        prefer_aur: bool,
    ) -> (Self, Task<crate::Message>) {
        let first = names.first().cloned().expect("start_batch needs a target");
        let dry_targets = names.clone();
        let model =
            TransactionModel::batch(first, names, aur_names, prefer_aur, InstallKind::Install);
        let task = crate::components::task::blocking_task(
            move || {
                let request = pakajo::dispatch::InstallRequest {
                    targets: dry_targets,
                    as_deps: false,
                    no_check: false,
                    ignores: vec![],
                    prefer_aur,
                    decider: Box::new(AutomaticDecider::new()),
                    approvals: None,
                    tty: false,
                    json: false,
                };
                pakajo::dispatch::install_preview(&request).map(|preview| preview.questions)
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
        let dry_name = name.clone();
        let model = TransactionModel::new(name, source, InstallKind::Remove);
        let task = crate::components::task::blocking_task(
            move || {
                let request = pakajo::dispatch::RemoveRequest {
                    targets: vec![dry_name],
                    tty: false,
                    json: false,
                    approvals: None,
                };
                pakajo::dispatch::preview(&request).map(|preview| preview.questions)
            },
            "dry-run channel closed",
            |result| crate::Message::Transaction(TransactionMessage::DryRunResult(result)).into(),
        );
        (Self { model }, task)
    }

    pub(crate) fn update(&mut self, message: TransactionMessage) -> Action {
        match message {
            TransactionMessage::StartInstall => Action::None,
            TransactionMessage::StartBatchInstall => Action::None,
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
                Err(e) if self.model.kind == InstallKind::Remove => {
                    eprintln!("[pakajo] remove dry-run failed: {e}");
                    self.model.finish(ChildOutcome::Failed(e));
                    Action::None
                }
                Ok(qs) if self.model.kind == InstallKind::Remove => {
                    if qs.held.is_empty() {
                        return self.launch_remove_subprocess(None);
                    }
                    eprintln!("[pakajo] held review required ({} held)", qs.held.len(),);
                    let review = ReviewModel::new(qs);
                    self.model.review = Some(review);
                    Action::None
                }
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
                if self.model.kind == InstallKind::Remove {
                    let review = match self.model.review.take() {
                        Some(r) => r,
                        None => return Action::None,
                    };
                    let approvals = review_approvals(&review);
                    return self.launch_remove_subprocess(approvals);
                }
                let mut review = match self.model.review.take() {
                    Some(r) => r,
                    None => return Action::None,
                };
                let approvals = review_approvals(&review);
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
                    for entry in &p.entries {
                        let _ = mark_seen(&entry.dir);
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
        if !self.model.aur_names.is_empty() {
            let targets = self.model.aur_names.clone();
            let task = crate::components::task::blocking_task(
                move || prepare_pkgbuild_diffs(&targets, false),
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
        let targets = self.model.targets.clone();
        let prefer_aur = self.model.prefer_aur;
        self.model.status = TransactionStatus::Running;
        let decider = match AutomaticDecider::from_payload(approvals.as_deref()) {
            Ok(decider) => Box::new(decider),
            Err(error) => {
                self.model
                    .finish(ChildOutcome::Failed(format!("{error:#}")));
                return Action::None;
            }
        };
        let request = pakajo::dispatch::InstallRequest {
            targets,
            as_deps: false,
            no_check: false,
            ignores: vec![],
            prefer_aur,
            decider,
            approvals,
            tty: false,
            json: false,
        };
        Action::Run(stream_items(pakajo::dispatch::install(request)))
    }

    fn launch_remove_subprocess(&mut self, approvals: Option<String>) -> Action {
        let name = self.model.name.clone();
        self.model.status = TransactionStatus::Running;
        let request = pakajo::dispatch::RemoveRequest {
            targets: vec![name],
            tty: false,
            json: false,
            approvals,
        };
        Action::Run(stream_items(pakajo::dispatch::remove(request)))
    }

    pub(crate) fn start_sysupgrade(
        request: pakajo::dispatch::SysupgradeRequest,
    ) -> (Self, Task<crate::Message>) {
        let mut transaction = Self {
            model: TransactionModel::new(
                "system".to_string(),
                PackageSource::Repo,
                InstallKind::Upgrade,
            ),
        };
        transaction.model.status = TransactionStatus::Running;
        (
            transaction,
            stream_items(pakajo::dispatch::sysupgrade(request)),
        )
    }

    pub(crate) fn view(&self) -> Element<'_> {
        if self.model.is_aur() {
            return aur::view(&self.model);
        }
        repo::view(&self.model)
    }

    pub(crate) fn dialog(&self) -> Option<Element<'_>> {
        let content = if let Some(r) = self.model.review.as_ref() {
            r.view(&self.model.name, self.model.kind)
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
