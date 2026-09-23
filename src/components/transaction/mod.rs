use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::Column;
use cosmic::widget::{button, container, dialog, text};
use futures::StreamExt as _;

use pakajo::dispatch::exec::{AnswerWriter, ChildOutcome, StreamItem};
use pakajo::dispatch::protocol::AutomaticDecider;
use pakajo::dispatch::revalidate::{RevalidationRun, ReviewLoop, ReviewStep};
use pakajo::events::InstallEvent;
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::progress::InstallKind;
use pakajo::question::model::Question;
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

pub(crate) mod checkout;
use checkout::CheckoutModel;

pub(crate) mod review;
use review::{InstallReview, ReviewMessage, ReviewModel};

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartBatchInstall,
    StartRemove,
    InstallEvent(InstallEvent),
    InstallDone(ChildOutcome),
    DryRunResult(Result<QuestionSet, String>),
    Explored(Result<RevalidationRun, String>),
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
    ApproveCheckout,
    CancelCheckout,
    AnswerChannel(AnswerWriter),
    AnswerImportKey(bool),
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

fn install_review_seal(review: &InstallReview) -> Option<String> {
    match review
        .seal()
        .and_then(|sealed| pakajo::dispatch::seal::encode_seal(&sealed))
    {
        Ok(payload) => Some(payload),
        Err(e) => {
            eprintln!("[pakajo] seal encoding failed: {e}");
            None
        }
    }
}

fn proceed_only_seal() -> String {
    match pakajo::dispatch::seal::proceed_only_seal() {
        Ok(payload) => payload,
        Err(e) => {
            eprintln!("[pakajo] seal encoding failed: {e}");
            String::new()
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
                let done = matches!(item, StreamItem::Done(_));
                let message = match item {
                    StreamItem::Event(ev) => TransactionMessage::InstallEvent(ev),
                    StreamItem::AnswerChannel(writer) => TransactionMessage::AnswerChannel(writer),
                    StreamItem::Done(outcome) => TransactionMessage::InstallDone(outcome),
                };
                let _ = tx.send(crate::Message::Transaction(message).into()).await;
                if done {
                    done_seen = true;
                    break;
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

fn review_stream(
    rx: std::sync::mpsc::Receiver<Result<RevalidationRun, String>>,
) -> Task<crate::Message> {
    Task::stream(channel(
        256,
        move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
            use futures::{SinkExt as _, StreamExt as _};
            let (bridge, mut forward) =
                futures::channel::mpsc::unbounded::<Result<RevalidationRun, String>>();
            std::thread::spawn(move || {
                while let Ok(result) = rx.recv() {
                    if bridge.unbounded_send(result).is_err() {
                        break;
                    }
                }
            });
            while let Some(result) = forward.next().await {
                let _ = tx
                    .send(crate::Message::Transaction(TransactionMessage::Explored(result)).into())
                    .await;
            }
            let _ = tx
                .send(
                    crate::Message::Transaction(TransactionMessage::Explored(Err(
                        "review loop ended".to_string(),
                    )))
                    .into(),
                )
                .await;
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
    _review_loop: Option<ReviewLoop>,
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
        let model = TransactionModel::batch(
            first,
            names.clone(),
            aur_names,
            prefer_aur,
            InstallKind::Install,
        );
        let request = pakajo::dispatch::InstallRequest {
            targets: names,
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: vec![],
            prefer_aur,
            decider: Box::new(AutomaticDecider::new()),
            approvals: None,
            tty: false,
            json: false,
        };
        let (review_loop, rx) = ReviewLoop::spawn(request);
        review_loop.send(ReviewStep::Defaults);
        (
            Self {
                model,
                _review_loop: Some(review_loop),
            },
            review_stream(rx),
        )
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
        (
            Self {
                model,
                _review_loop: None,
            },
            task,
        )
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
                Err(e) => {
                    eprintln!("[pakajo] remove dry-run failed: {e}");
                    self.model.finish(ChildOutcome::Failed(e));
                    Action::None
                }
                Ok(qs) => {
                    if qs.held.is_empty() {
                        return self.launch_remove_subprocess(None);
                    }
                    eprintln!("[pakajo] held review required ({} held)", qs.held.len(),);
                    let review = ReviewModel::new(qs);
                    self.model.review = Some(review);
                    Action::None
                }
            },
            TransactionMessage::Explored(result) => match result {
                Err(e) => {
                    eprintln!("[pakajo] review failed: {e}");
                    self.model.failure_message = Some(e);
                    Action::None
                }
                Ok(run) => {
                    self.model.summary = Some(run.summary);
                    if !review::install_needs_review(&run.questions) {
                        eprintln!("[pakajo] no questions, showing checkout");
                        return self.show_checkout();
                    }
                    eprintln!(
                        "[pakajo] review required ({} questions)",
                        run.questions.len()
                    );
                    self.model.install_review = Some(InstallReview::new(run.questions));
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
                    r.update(m.clone());
                }
                if let Some(r) = self.model.install_review.as_mut() {
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
                let mut review = match self.model.install_review.take() {
                    Some(r) => r,
                    None => return Action::None,
                };
                let approvals = install_review_seal(&review);
                if matches!(self.model.source, PackageSource::Aur) {
                    review.approving = true;
                    self.model.install_review = Some(review);
                }
                self.proceed_after_conflicts(approvals)
            }
            TransactionMessage::PkgbuildResult(result) => {
                self.model.review = None;
                self.model.install_review = None;
                match result {
                    Err(e) => {
                        eprintln!("[pakajo] pkgbuild fetch failed, showing checkout: {e}");
                        self.show_checkout()
                    }
                    Ok(diffs) if diffs.is_empty() => self.show_checkout(),
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
                self.launch_subprocess(approvals.unwrap_or_else(proceed_only_seal))
            }
            TransactionMessage::CancelPkgbuild => Action::Finished,
            TransactionMessage::ApproveCheckout => {
                if self.model.checkout.take().is_none() {
                    return Action::None;
                }
                let approvals = self.model.pending_approvals.take();
                self.launch_subprocess(approvals.unwrap_or_else(proceed_only_seal))
            }
            TransactionMessage::CancelCheckout => Action::Finished,
            TransactionMessage::AnswerChannel(writer) => {
                self.model.set_answer_channel(writer);
                Action::None
            }
            TransactionMessage::AnswerImportKey(yes) => {
                self.model.answer_import_key(yes);
                Action::None
            }
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
            self.show_checkout()
        }
    }

    fn show_checkout(&mut self) -> Action {
        let summary = self.model.summary.clone().unwrap_or_default();
        self.model.checkout = Some(CheckoutModel::new(summary));
        Action::None
    }

    fn launch_subprocess(&mut self, approvals: String) -> Action {
        let targets = self.model.targets.clone();
        let prefer_aur = self.model.prefer_aur;
        self.model.status = TransactionStatus::Running;
        let decider: Box<dyn pakajo::dispatch::protocol::Decider + Send> =
            match pakajo::dispatch::seal::decode_seal(&approvals) {
                Ok(sealed) => Box::new(pakajo::dispatch::seal::sealed_decider(sealed)),
                Err(error) => {
                    self.model
                        .finish(ChildOutcome::Failed(format!("{error:#}")));
                    return Action::None;
                }
            };
        let request = pakajo::dispatch::InstallRequest {
            targets,
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: vec![],
            prefer_aur,
            decider,
            approvals: Some(approvals),
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
            _review_loop: None,
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
        if let Some(prompt) = self.model.pending_import_key.as_ref() {
            return Some(dialog_backdrop(import_key_dialog(prompt), 32.0));
        }
        let content = if let Some(r) = self.model.install_review.as_ref() {
            r.view(&self.model.name)
        } else if let Some(c) = self.model.checkout.as_ref() {
            c.view(&self.model.name)
        } else if let Some(r) = self.model.review.as_ref() {
            r.view(&self.model.name, self.model.kind)
        } else {
            self.model.pkgbuild_review.as_ref().map(|p| p.view())?
        };
        Some(dialog_backdrop(content, 32.0))
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

fn import_key_dialog(prompt: &Question) -> Element<'_> {
    let Question::ImportKey { fingerprint, uid } = prompt else {
        return dialog_backdrop(text("Unknown prompt").into(), 32.0);
    };
    let mut body = Column::new()
        .spacing(8)
        .push(text(format!("Import PGP key {fingerprint}?")));
    if !uid.is_empty() {
        body = body.push(text(uid.clone()));
    }
    dialog()
        .title("Import PGP key")
        .control(body)
        .primary_action(
            button::suggested("Yes").on_press(crate::Message::Transaction(
                TransactionMessage::AnswerImportKey(true),
            )),
        )
        .secondary_action(button::standard("No").on_press(crate::Message::Transaction(
            TransactionMessage::AnswerImportKey(false),
        )))
        .into()
}

fn dialog_backdrop(content: Element<'_>, padding: f32) -> Element<'_> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(padding)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.55))),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pakajo::dispatch::revalidate::ReviewOrigin;
    use pakajo::events::{SummaryPackage, TransactionSummary};

    fn test_summary() -> TransactionSummary {
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

    fn reviewed() -> RevalidationRun {
        RevalidationRun {
            origin: ReviewOrigin::Initial,
            questions: vec![Question::InstallIgnorepkg { name: s("glibc") }],
            answers: Vec::new(),
            summary: test_summary(),
        }
    }

    fn batch_transaction() -> Transaction {
        Transaction {
            model: TransactionModel::batch(
                s("paru"),
                vec![s("paru")],
                vec![s("paru")],
                false,
                InstallKind::Install,
            ),
            _review_loop: None,
        }
    }

    #[test]
    fn pkgbuild_result_clears_install_review() {
        let mut transaction = batch_transaction();
        transaction.model.install_review =
            Some(InstallReview::new(vec![Question::InstallIgnorepkg {
                name: s("glibc"),
            }]));

        transaction.update(TransactionMessage::PkgbuildResult(Ok(vec![])));

        assert!(transaction.model.install_review.is_none());
    }

    #[test]
    fn explored_run_with_questions_shows_review() {
        let mut transaction = batch_transaction();
        transaction.update(TransactionMessage::Explored(Ok(reviewed())));
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.checkout.is_none());
        assert_eq!(
            transaction.model.summary.clone().expect("summary set"),
            test_summary()
        );
    }

    #[test]
    fn explored_run_without_questions_shows_checkout() {
        let mut transaction = batch_transaction();
        let empty = RevalidationRun {
            origin: ReviewOrigin::Initial,
            questions: Vec::new(),
            answers: Vec::new(),
            summary: test_summary(),
        };
        transaction.update(TransactionMessage::Explored(Ok(empty)));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_some());
    }

    #[test]
    fn explored_error_fails_closed_without_review_or_checkout() {
        let mut transaction = batch_transaction();
        transaction.update(TransactionMessage::Explored(Err(s("boom"))));
        assert_eq!(transaction.model.failure_message.as_deref(), Some("boom"));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(transaction.model.pending_approvals.is_none());
    }

    fn s(value: &str) -> String {
        value.to_string()
    }
}
