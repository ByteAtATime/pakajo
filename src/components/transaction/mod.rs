use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::Column;
use cosmic::widget::{button, container, dialog, text};
use futures::StreamExt as _;

use pakajo::dispatch::exec::{AnswerWriter, ChildOutcome, StreamItem};
use pakajo::dispatch::protocol::AutomaticDecider;
use pakajo::dispatch::revalidate::{
    RevalidationRun, ReviewLoop, ReviewOrigin, ReviewPlan, ReviewStep,
};
use pakajo::events::InstallEvent;
use pakajo::package::PackageSource;
use pakajo::pkgbuild::{PkgbuildDiff, mark_seen, prepare_pkgbuild_diffs};
use pakajo::progress::InstallKind;
use pakajo::question::model::Question;
use pakajo::question::revalidate::{Verdict, converge, revalidate};

use crate::Element;

pub(super) const SYSTEM_AUR_NAME: &str = "system-aur";

const REVIEW_LOOP_ENDED: &str = "review loop ended";

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
use review::{InstallReview, ReviewMessage};

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartBatchInstall,
    StartRemove,
    InstallEvent(InstallEvent),
    InstallDone(ChildOutcome),
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
                        REVIEW_LOOP_ENDED.to_string(),
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
    review_loop: Option<ReviewLoop>,
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
        let (review_loop, rx) = ReviewLoop::spawn(ReviewPlan::Install(Box::new(request)));
        review_loop.send(ReviewStep::Defaults);
        (
            Self {
                model,
                review_loop: Some(review_loop),
            },
            review_stream(rx),
        )
    }

    pub(crate) fn start_remove(
        name: String,
        source: PackageSource,
    ) -> (Self, Task<crate::Message>) {
        let targets = vec![name.clone()];
        let model = TransactionModel::new(name, source, InstallKind::Remove);
        let holds = pakajo::pacman::config()
            .map(|config| config.hold_pkg)
            .unwrap_or_default();
        let (review_loop, rx) = ReviewLoop::spawn(ReviewPlan::Remove { targets, holds });
        review_loop.send(ReviewStep::Defaults);
        (
            Self {
                model,
                review_loop: Some(review_loop),
            },
            review_stream(rx),
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
            TransactionMessage::Explored(result) => match result {
                Err(e) => {
                    if e == REVIEW_LOOP_ENDED {
                        if self.model.checkout.is_some() {
                            return Action::None;
                        }
                        let approving = self
                            .model
                            .install_review
                            .as_ref()
                            .is_some_and(|review| review.approving);
                        if approving {
                            eprintln!("[pakajo] review failed: {e}");
                            self.model.failure_message = Some(e);
                            self.model.pending_approvals = None;
                            if let Some(review) = self.model.install_review.as_mut() {
                                review.approving = false;
                            }
                            return Action::None;
                        }
                        return Action::None;
                    }
                    eprintln!("[pakajo] review failed: {e}");
                    self.model.failure_message = Some(e);
                    self.model.pending_approvals = None;
                    if let Some(review) = self.model.install_review.as_mut() {
                        review.approving = false;
                    }
                    Action::None
                }
                Ok(run) => {
                    if run.origin == ReviewOrigin::Revalidation {
                        return self.apply_revalidation(run);
                    }
                    self.accept_initial(run)
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
                if self
                    .model
                    .install_review
                    .as_ref()
                    .is_some_and(|review| review.approving)
                {
                    return Action::None;
                }
                if let Some(r) = self.model.install_review.as_mut() {
                    r.update(m);
                }
                Action::None
            }
            TransactionMessage::CancelReview => Action::Finished,
            TransactionMessage::ApproveReview => {
                if self
                    .model
                    .install_review
                    .as_ref()
                    .is_some_and(|review| review.approving)
                {
                    return Action::None;
                }
                self.model.review_notice = None;
                self.model.unstables = 0;
                if self
                    .model
                    .install_review
                    .as_ref()
                    .is_some_and(|review| !review.can_confirm())
                {
                    return Action::None;
                }
                let mut review = match self.model.install_review.take() {
                    Some(r) => r,
                    None => return Action::None,
                };
                let sealed = match review.seal() {
                    Ok(sealed) => sealed,
                    Err(e) => return self.fail_seal(review, format!("{e:#}")),
                };
                let approvals = match pakajo::dispatch::seal::encode_seal(&sealed) {
                    Ok(payload) => payload,
                    Err(e) => return self.fail_seal(review, format!("{e:#}")),
                };
                review.approving = true;
                self.model.install_review = Some(review);
                self.model.pending_approvals = Some(approvals);
                if let Some(review_loop) = self.review_loop.as_ref()
                    && !review_loop.send(ReviewStep::Sealed(sealed))
                {
                    self.model.failure_message = Some(REVIEW_LOOP_ENDED.to_string());
                    self.model.pending_approvals = None;
                    if let Some(review) = self.model.install_review.as_mut() {
                        review.approving = false;
                    }
                }
                Action::None
            }
            TransactionMessage::PkgbuildResult(result) => {
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
                let payload = approvals.unwrap_or_else(proceed_only_seal);
                if self.model.kind == InstallKind::Remove {
                    return self.launch_remove_subprocess(payload);
                }
                self.launch_subprocess(payload)
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

    fn fail_seal(&mut self, mut review: InstallReview, message: String) -> Action {
        eprintln!("[pakajo] seal encoding failed: {message}");
        self.model.failure_message = Some(message);
        review.approving = false;
        self.model.install_review = Some(review);
        Action::None
    }

    fn accept_initial(&mut self, run: RevalidationRun) -> Action {
        self.model.summary = Some(run.summary);
        self.model.revalidations = 0;
        self.model.unstables = 0;
        self.model.pending_approvals = None;
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

    fn apply_revalidation(&mut self, run: RevalidationRun) -> Action {
        let outcome = match (
            self.model.install_review.as_ref(),
            self.model.summary.as_ref(),
        ) {
            (Some(review), Some(summary)) => revalidate(
                &review.questions,
                &review.answers(),
                summary,
                &run.questions,
                &run.answers,
                &run.summary,
            ),
            _ => {
                eprintln!("[pakajo] revalidation without review or summary");
                return Action::None;
            }
        };
        match converge(self.model.revalidations, &outcome) {
            Verdict::Converged => {
                self.model.summary = Some(run.summary);
                self.model.review_notice = None;
                self.model.install_review.take();
                match self.model.pending_approvals.take() {
                    Some(payload) => self.proceed_after_conflicts(Some(payload)),
                    None => {
                        eprintln!("[pakajo] revalidation converged without pending approvals");
                        self.model.failure_message =
                            Some("revalidation converged without pending approvals".to_string());
                        Action::None
                    }
                }
            }
            Verdict::Diverged(drift) => {
                self.model.revalidations += 1;
                self.model.summary = Some(run.summary);
                self.model.pending_approvals = None;
                match self.model.install_review.as_mut() {
                    Some(review) => review.refresh(run.questions, &drift),
                    None => {
                        self.model.install_review = Some(InstallReview::new(run.questions));
                    }
                }
                eprintln!(
                    "[pakajo] review drifted ({} added, {} changed, {} removed, {} summary added, {} summary removed)",
                    drift.added.len(),
                    drift.changed.len(),
                    drift.removed.len(),
                    drift.summary.added.len(),
                    drift.summary.removed.len()
                );
                Action::None
            }
            Verdict::Unstable => {
                self.model.revalidations = 0;
                self.model.pending_approvals = None;
                self.model.unstables += 1;
                if self.model.unstables >= 2 {
                    let message = "review did not stabilize".to_string();
                    eprintln!("[pakajo] {message}");
                    self.model.failure_message = Some(message);
                    self.model.review_notice = Some("review did not stabilize".to_string());
                    if let Some(review) = self.model.install_review.as_mut() {
                        review.approving = false;
                    }
                    return Action::None;
                }
                self.model.review_notice = Some("review did not stabilize".to_string());
                eprintln!("[pakajo] review did not stabilize");
                if let Some(review_loop) = self.review_loop.as_ref() {
                    review_loop.send(ReviewStep::Defaults);
                }
                Action::None
            }
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
        self.model.checkout = Some(CheckoutModel::new(summary, self.model.kind));
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

    fn launch_remove_subprocess(&mut self, approvals: String) -> Action {
        let targets = self.model.targets.clone();
        self.model.status = TransactionStatus::Running;
        let request = pakajo::dispatch::RemoveRequest {
            targets,
            tty: false,
            json: false,
            approvals: Some(approvals),
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
            review_loop: None,
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
            r.view(&self.model.name, self.model.kind)
        } else if let Some(c) = self.model.checkout.as_ref() {
            c.view(&self.model.name)
        } else {
            self.model.pkgbuild_review.as_ref().map(|p| p.view())?
        };
        let content = match self.model.review_notice.as_ref() {
            Some(notice) => Column::new()
                .push(text(notice.clone()))
                .push(content)
                .into(),
            None => content,
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
            answers: vec![ignorepkg_answer("glibc")],
            summary: test_summary(),
            aur: Vec::new(),
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
            review_loop: None,
        }
    }

    fn repo_transaction() -> Transaction {
        Transaction {
            model: TransactionModel::batch(
                s("firefox"),
                vec![s("firefox")],
                Vec::new(),
                false,
                InstallKind::Install,
            ),
            review_loop: None,
        }
    }

    fn ignorepkg_answer(name: &str) -> pakajo::question::model::Answer {
        pakajo::question::model::Answer::InstallIgnorepkg {
            name: s(name),
            install: false,
        }
    }

    fn revalidation_run(
        questions: Vec<Question>,
        answers: Vec<pakajo::question::model::Answer>,
    ) -> RevalidationRun {
        RevalidationRun {
            origin: ReviewOrigin::Revalidation,
            questions,
            answers,
            summary: test_summary(),
            aur: Vec::new(),
        }
    }

    fn approving_transaction(pending: Option<String>, revalidations: usize) -> Transaction {
        let mut transaction = repo_transaction();
        let mut review = InstallReview::new(vec![Question::InstallIgnorepkg { name: s("glibc") }]);
        review.approving = true;
        transaction.model.install_review = Some(review);
        transaction.model.summary = Some(test_summary());
        transaction.model.pending_approvals = pending;
        transaction.model.revalidations = revalidations;
        transaction
    }

    fn drifted_run() -> RevalidationRun {
        revalidation_run(
            vec![
                Question::InstallIgnorepkg { name: s("glibc") },
                Question::InstallIgnorepkg { name: s("nvidia") },
            ],
            vec![ignorepkg_answer("glibc"), ignorepkg_answer("nvidia")],
        )
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
            aur: Vec::new(),
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

    #[test]
    fn approve_seals_and_waits_for_revalidation() {
        let mut transaction = repo_transaction();
        transaction.model.install_review =
            Some(InstallReview::new(vec![Question::InstallIgnorepkg {
                name: s("glibc"),
            }]));
        transaction.update(TransactionMessage::ApproveReview);
        let review = transaction
            .model
            .install_review
            .as_ref()
            .expect("review kept in place");
        assert!(review.approving);
        assert!(transaction.model.pending_approvals.is_some());
        assert!(transaction.model.checkout.is_none());
    }

    #[test]
    fn revalidation_diverges_then_converges_to_checkout() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 0);
        if let Some(review) = transaction.model.install_review.as_mut() {
            review.ignorepkg_checks[0] = true;
        }
        transaction.update(TransactionMessage::Explored(Ok(drifted_run())));
        assert_eq!(transaction.model.revalidations, 1);
        let rebuilt = transaction
            .model
            .install_review
            .as_ref()
            .expect("review rebuilt");
        assert_eq!(rebuilt.questions.len(), 2);
        assert!(!rebuilt.approving);
        assert!(rebuilt.ignorepkg_checks[0]);
        assert!(!rebuilt.ignorepkg_checks[1]);
        assert_eq!(rebuilt.highlighted.len(), 1);
        assert!(
            rebuilt
                .highlighted
                .contains(&Question::InstallIgnorepkg { name: s("nvidia") }.key())
        );

        assert!(transaction.model.pending_approvals.is_none());
        transaction.model.install_review = Some({
            let mut review =
                InstallReview::new(vec![Question::InstallIgnorepkg { name: s("glibc") }]);
            review.approving = true;
            review
        });
        transaction.model.pending_approvals = Some(s("sealed-payload"));
        let settled = revalidation_run(
            vec![Question::InstallIgnorepkg { name: s("glibc") }],
            vec![ignorepkg_answer("glibc")],
        );
        transaction.update(TransactionMessage::Explored(Ok(settled)));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_some());
        assert_eq!(
            transaction.model.pending_approvals.as_deref(),
            Some("sealed-payload")
        );
    }

    #[test]
    fn unstable_notice_survives_initial_restart() {
        let mut transaction = approving_transaction(None, 2);
        transaction.update(TransactionMessage::Explored(Ok(drifted_run())));
        assert_eq!(transaction.model.revalidations, 0);
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );

        transaction.update(TransactionMessage::Explored(Ok(reviewed())));
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );
        assert!(transaction.model.install_review.is_some());

        transaction.update(TransactionMessage::Explored(Err(
            REVIEW_LOOP_ENDED.to_string()
        )));
        assert!(transaction.model.failure_message.is_none());
        assert!(transaction.model.install_review.is_some());
    }

    #[test]
    fn diverged_with_summary_only_drift_still_rebuilds() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 0);
        let mut varied = test_summary();
        varied.packages[0].new_version = s("2.0");
        let run = RevalidationRun {
            origin: ReviewOrigin::Revalidation,
            questions: vec![Question::InstallIgnorepkg { name: s("glibc") }],
            answers: vec![ignorepkg_answer("glibc")],
            summary: varied,
            aur: Vec::new(),
        };
        transaction.update(TransactionMessage::Explored(Ok(run)));
        assert_eq!(transaction.model.revalidations, 1);
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.pending_approvals.is_none());
    }

    #[test]
    fn sentinel_while_approving_fails_closed() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 0);
        transaction.update(TransactionMessage::Explored(Err(
            REVIEW_LOOP_ENDED.to_string()
        )));
        assert_eq!(
            transaction.model.failure_message.as_deref(),
            Some(REVIEW_LOOP_ENDED)
        );
        assert!(
            transaction
                .model
                .install_review
                .as_ref()
                .is_some_and(|review| !review.approving)
        );
    }

    #[test]
    fn unstable_clears_stash_and_notice_survives_restart() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 2);
        transaction.update(TransactionMessage::Explored(Ok(drifted_run())));
        assert!(transaction.model.pending_approvals.is_none());
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );
        transaction.update(TransactionMessage::Explored(Ok(reviewed())));
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );
        assert!(transaction.model.pending_approvals.is_none());
    }

    #[test]
    fn second_consecutive_unstable_fails_closed() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 2);
        transaction.model.unstables = 1;
        transaction.update(TransactionMessage::Explored(Ok(drifted_run())));
        assert!(
            transaction
                .model
                .failure_message
                .as_deref()
                .is_some_and(|message| message.contains("review did not stabilize"))
        );
        assert!(
            transaction
                .model
                .install_review
                .as_ref()
                .is_some_and(|review| !review.approving)
        );
        assert!(transaction.model.install_review.is_some());
    }

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn remove_transaction() -> Transaction {
        Transaction {
            model: TransactionModel::new(s("firefox"), PackageSource::Repo, InstallKind::Remove),
            review_loop: None,
        }
    }

    fn removal_summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![SummaryPackage {
                name: s("firefox"),
                repository: None,
                new_version: s("1.0"),
                old_version: Some(s("1.0")),
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

    fn remove_questions() -> Vec<Question> {
        vec![
            Question::HoldPkgs {
                names: vec![s("firefox")],
            },
            Question::RemovePkgs {
                names: vec![s("ghost")],
                kind: pakajo::question::model::TransactionKind::Remove,
            },
        ]
    }

    fn initial_remove_run(questions: Vec<Question>) -> RevalidationRun {
        RevalidationRun {
            origin: ReviewOrigin::Initial,
            questions,
            answers: Vec::new(),
            summary: removal_summary(),
            aur: Vec::new(),
        }
    }

    #[test]
    fn remove_empty_part1_goes_straight_to_checkout() {
        let mut transaction = remove_transaction();
        transaction.update(TransactionMessage::Explored(Ok(initial_remove_run(
            Vec::new(),
        ))));
        assert!(transaction.model.install_review.is_none());
        let checkout = transaction.model.checkout.as_ref().expect("checkout shown");
        assert_eq!(checkout.summary.packages.len(), 1);
        assert!(
            checkout
                .summary
                .packages
                .iter()
                .all(|package| package.is_removal)
        );
        assert_eq!(checkout.summary, removal_summary());
    }

    #[test]
    fn remove_review_gates_confirm_until_hold_acknowledged() {
        let mut transaction = remove_transaction();
        transaction.update(TransactionMessage::Explored(Ok(initial_remove_run(
            remove_questions(),
        ))));
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.checkout.is_none());
        transaction.update(TransactionMessage::ApproveReview);
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.pending_approvals.is_none());
        assert!(transaction.model.checkout.is_none());
    }

    #[test]
    fn remove_sealed_revalidation_converges_to_checkout_and_launches() {
        let mut transaction = remove_transaction();
        transaction.update(TransactionMessage::Explored(Ok(initial_remove_run(
            remove_questions(),
        ))));
        transaction.update(TransactionMessage::Review(ReviewMessage::ToggleHoldpkgs(0)));
        transaction.update(TransactionMessage::ApproveReview);
        let payload = transaction
            .model
            .pending_approvals
            .clone()
            .expect("sealed payload");
        let sealed = pakajo::dispatch::seal::decode_seal(&payload).expect("decodes");
        assert!(sealed.proceed);
        assert!(sealed.answers.iter().any(|(_, answer)| matches!(
            answer,
            pakajo::question::model::Answer::HoldPkgs { names, proceed: true }
            if names == &vec![s("firefox")]
        )));
        let review = transaction
            .model
            .install_review
            .as_ref()
            .expect("review kept");
        assert!(review.approving);
        let settled = RevalidationRun {
            origin: ReviewOrigin::Revalidation,
            questions: review.questions.clone(),
            answers: review.answers(),
            summary: removal_summary(),
            aur: Vec::new(),
        };
        transaction.update(TransactionMessage::Explored(Ok(settled)));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_some());
        let carried = transaction
            .model
            .pending_approvals
            .clone()
            .expect("payload carried");
        let decoded = pakajo::dispatch::seal::decode_seal(&carried).expect("decodes");
        assert!(decoded.proceed);
        transaction.update(TransactionMessage::ApproveCheckout);
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));
    }
}
