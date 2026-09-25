use cosmic::app::Task;
use cosmic::iced::{Background, Color, Length, stream::channel};
use cosmic::widget::Column;
use cosmic::widget::{button, container, dialog, text};
use futures::StreamExt as _;

use pakajo::dispatch::exec::{AnswerWriter, ChildOutcome, StreamItem};
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
use crate::PakajoCtx;

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

pub(crate) mod diff;
mod pkgbuild;
use pkgbuild::{PkgbuildMessage, PkgbuildModel};

pub(crate) mod checkout;
use checkout::CheckoutModel;

pub(crate) mod review;
use review::{InstallReview, ReviewMessage};

pub type OptDepSelection = Vec<(String, String)>;

#[derive(Clone, Debug)]
pub enum TransactionRequest {
    Install {
        name: String,
        source: PackageSource,
        with_deps: Vec<(String, String)>,
    },
    BatchInstall(OptDepSelection),
    Remove {
        name: String,
        source: PackageSource,
    },
}

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    Begin(TransactionRequest),
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

fn sealed_decider(
    approvals: &str,
) -> Result<Box<dyn pakajo::dispatch::protocol::Decider + Send>, String> {
    pakajo::dispatch::seal::decode_seal(approvals)
        .map(|sealed| Box::new(pakajo::dispatch::seal::sealed_decider(sealed)) as Box<_>)
        .map_err(|error| format!("{error:#}"))
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

#[derive(Default)]
pub struct TxPane {
    pub(crate) transaction: Option<Transaction>,
    pub(crate) show: bool,
}

impl TxPane {
    pub fn begin(&mut self, request: TransactionRequest, ctx: &PakajoCtx) -> Task<crate::Message> {
        if self.is_active() {
            return Task::none();
        }
        match request {
            TransactionRequest::Install {
                name,
                source,
                with_deps,
            } => {
                if with_deps.is_empty() {
                    let (txn, task) = Transaction::start(name, source);
                    self.transaction = Some(txn);
                    self.show = false;
                    return task;
                }
                let wanted = std::iter::once((name, String::new()))
                    .chain(with_deps)
                    .collect();
                self.start_optdep_batch(wanted, ctx)
            }
            TransactionRequest::BatchInstall(with_deps) => {
                if with_deps.is_empty() {
                    return Task::none();
                }
                self.start_optdep_batch(with_deps, ctx)
            }
            TransactionRequest::Remove { name, source } => {
                let (txn, task) = Transaction::start_remove(name, source);
                self.transaction = Some(txn);
                self.show = false;
                task
            }
        }
    }

    fn start_optdep_batch(
        &mut self,
        wanted: OptDepSelection,
        ctx: &PakajoCtx,
    ) -> Task<crate::Message> {
        let resolve = |dep: &str, constraint: &str| {
            ctx.alpm
                .as_ref()
                .and_then(|h| h.syncdbs().find_satisfier(format!("{dep}{constraint}")))
                .map(|p| p.name().to_string())
        };
        let (targets, aur_bucket) = partition_batch_targets(&wanted, resolve);
        let (txn, task) = Transaction::start_batch(targets, aur_bucket);
        self.transaction = Some(txn);
        self.show = false;
        task
    }

    pub fn start_sysupgrade(&mut self) -> Task<crate::Message> {
        if self.is_active() {
            return Task::none();
        }
        let (transaction, task) = Transaction::start_sysupgrade();
        self.transaction = Some(transaction);
        task
    }

    pub fn forward(&mut self, message: TransactionMessage) -> Action {
        match self.transaction.as_mut() {
            Some(t) => t.update(message),
            None => Action::None,
        }
    }

    pub fn finish(&mut self) {
        self.transaction = None;
    }

    pub fn close(&mut self) {
        self.show = false;
    }

    pub fn open(&mut self) {
        self.show = true;
    }

    pub fn is_active(&self) -> bool {
        self.transaction
            .as_ref()
            .is_some_and(Transaction::is_active)
    }

    pub fn active_name(&self) -> Option<&str> {
        self.transaction
            .as_ref()
            .filter(|t| t.is_active())
            .map(Transaction::name)
    }

    pub fn sysupgrade_checking(&self) -> bool {
        self.transaction
            .as_ref()
            .is_some_and(|t| t.is_sysupgrade() && t.is_checking())
    }

    pub fn sysupgrade_running(&self) -> bool {
        self.transaction.as_ref().is_some_and(|t| t.is_sysupgrade())
    }

    pub fn building(&self) -> bool {
        self.transaction
            .as_ref()
            .is_some_and(|t| t.is_active() && t.building())
    }

    pub fn overlay(&self) -> Option<&Transaction> {
        self.transaction.as_ref().filter(|t| {
            (t.is_sysupgrade() && !t.is_checking()) || (self.show && !t.is_sysupgrade())
        })
    }

    pub fn dialog(&self) -> Option<Element<'_>> {
        self.transaction.as_ref().and_then(Transaction::dialog)
    }
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
        Self::start_batch(targets, aur_names)
    }

    pub(crate) fn start_batch(
        names: Vec<String>,
        aur_names: Vec<String>,
    ) -> (Self, Task<crate::Message>) {
        let first = names.first().cloned().expect("start_batch needs a target");
        let model = TransactionModel::batch(first, names.clone(), aur_names, InstallKind::Install);
        let request = pakajo::dispatch::InstallRequest {
            targets: names,
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: vec![],
            decider: pakajo::dispatch::seal::proceed_decider(),
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
            TransactionMessage::Begin(_) => Action::None,
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
                    if e == REVIEW_LOOP_ENDED && !self.approving() {
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
                if self.approving() {
                    return Action::None;
                }
                if let Some(r) = self.model.install_review.as_mut() {
                    r.update(m);
                }
                Action::None
            }
            TransactionMessage::CancelReview => Action::Finished,
            TransactionMessage::ApproveReview => {
                if self.approving() {
                    return Action::None;
                }
                self.model.review_notice = None;
                self.model.unstables = 0;
                if !self
                    .model
                    .install_review
                    .as_ref()
                    .is_some_and(|review| review.can_confirm())
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
                        eprintln!("[pakajo] pkgbuild fetch failed, starting transaction: {e}");
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
                self.launch_remove_subprocess(payload)
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
        }
    }

    fn approving(&self) -> bool {
        self.model
            .install_review
            .as_ref()
            .is_some_and(|review| review.approving)
    }

    fn fail_launch(&mut self, error: String) -> Action {
        self.model.finish(ChildOutcome::Failed(error));
        Action::None
    }

    fn fail_seal(&mut self, mut review: InstallReview, message: String) -> Action {
        eprintln!("[pakajo] seal encoding failed: {message}");
        self.model.failure_message = Some(message);
        review.approving = false;
        self.model.install_review = Some(review);
        Action::None
    }

    fn accept_initial(&mut self, run: RevalidationRun) -> Action {
        let upgrade = self.model.kind == InstallKind::Upgrade;
        let idle_upgrade = upgrade
            && run.questions.is_empty()
            && run.summary.packages.is_empty()
            && run.aur.is_empty();
        if upgrade {
            self.model.adopt_aur_candidates(run.aur);
        }
        self.model.summary = Some(run.summary);
        self.model.revalidations = 0;
        self.model.unstables = 0;
        self.model.pending_approvals = None;
        if idle_upgrade {
            eprintln!("[pakajo] sysupgrade found nothing to do");
            self.model.status = TransactionStatus::Done(ChildOutcome::Success);
            self.model.review_notice = Some("System is up to date".to_string());
            return Action::None;
        }
        if !review::install_needs_review(&run.questions) {
            eprintln!("[pakajo] no questions, starting transaction");
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
                if self.model.kind == InstallKind::Upgrade {
                    self.model.adopt_aur_candidates(run.aur);
                }
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
        if self.model.kind == InstallKind::Remove {
            let summary = self.model.summary.clone().unwrap_or_default();
            self.model.checkout = Some(CheckoutModel::new(summary));
            return Action::None;
        }
        eprintln!("[pakajo] no checkout needed, starting transaction");
        let payload = self
            .model
            .pending_approvals
            .take()
            .unwrap_or_else(proceed_only_seal);
        if self.model.kind == InstallKind::Upgrade {
            return self.launch_sysupgrade_subprocess(payload);
        }
        self.launch_subprocess(payload)
    }

    fn launch_subprocess(&mut self, approvals: String) -> Action {
        let targets = self.model.targets.clone();
        self.model.status = TransactionStatus::Running;
        let decider = match sealed_decider(&approvals) {
            Ok(decider) => decider,
            Err(error) => return self.fail_launch(error),
        };
        let request = pakajo::dispatch::InstallRequest {
            targets,
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: vec![],
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

    fn launch_sysupgrade_subprocess(&mut self, approvals: String) -> Action {
        self.model.status = TransactionStatus::Running;
        let decider = match sealed_decider(&approvals) {
            Ok(decider) => decider,
            Err(error) => return self.fail_launch(error),
        };
        let request = pakajo::dispatch::SysupgradeRequest {
            no_refresh: false,
            repo_only: false,
            ignores: Vec::new(),
            decider,
            aur_targets: Some(self.model.aur_names.clone()),
            approvals: Some(approvals),
            tty: false,
            json: false,
            print_nothing_to_do: false,
        };
        Action::Run(stream_items(pakajo::dispatch::sysupgrade(request)))
    }

    pub(crate) fn start_sysupgrade() -> (Self, Task<crate::Message>) {
        let model = TransactionModel::new(
            "system".to_string(),
            PackageSource::Repo,
            InstallKind::Upgrade,
        );
        let (review_loop, rx) = ReviewLoop::spawn(ReviewPlan::Upgrade {
            no_refresh: false,
            ignores: Vec::new(),
        });
        review_loop.send(ReviewStep::Defaults);
        (
            Self {
                model,
                review_loop: Some(review_loop),
            },
            review_stream(rx),
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
    use pakajo::question::model::Answer;

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

    fn new_transaction(kind: InstallKind) -> Transaction {
        let name = match kind {
            InstallKind::Upgrade => s("system"),
            _ => s("firefox"),
        };
        Transaction {
            model: TransactionModel::batch(name.clone(), vec![name], Vec::new(), kind),
            review_loop: None,
        }
    }

    fn ignore_question(name: &str) -> Question {
        Question::InstallIgnorepkg { name: s(name) }
    }

    fn ignore_answer(name: &str) -> Answer {
        Answer::InstallIgnorepkg {
            name: s(name),
            install: false,
        }
    }

    fn run(
        origin: ReviewOrigin,
        questions: Vec<Question>,
        answers: Vec<Answer>,
        summary: TransactionSummary,
        aur: Vec<pakajo::upgrade::AurUpgradeCandidate>,
    ) -> RevalidationRun {
        RevalidationRun {
            origin,
            questions,
            answers,
            summary,
            aur,
        }
    }

    fn initial_run(questions: Vec<Question>) -> RevalidationRun {
        run(
            ReviewOrigin::Initial,
            questions,
            Vec::new(),
            summary(),
            Vec::new(),
        )
    }

    fn settled_run(questions: Vec<Question>, answers: Vec<Answer>) -> RevalidationRun {
        run(
            ReviewOrigin::Revalidation,
            questions,
            answers,
            summary(),
            Vec::new(),
        )
    }

    fn drifted_run() -> RevalidationRun {
        settled_run(
            vec![ignore_question("glibc"), ignore_question("nvidia")],
            vec![ignore_answer("glibc"), ignore_answer("nvidia")],
        )
    }

    fn approving_transaction(pending: Option<String>, revalidations: usize) -> Transaction {
        let mut transaction = new_transaction(InstallKind::Install);
        let mut review = InstallReview::new(vec![ignore_question("glibc")]);
        review.approving = true;
        transaction.model.install_review = Some(review);
        transaction.model.summary = Some(summary());
        transaction.model.pending_approvals = pending;
        transaction.model.revalidations = revalidations;
        transaction
    }

    fn aur_candidate(name: &str) -> pakajo::upgrade::AurUpgradeCandidate {
        pakajo::upgrade::AurUpgradeCandidate {
            name: s(name),
            local_version: s("1.0"),
            remote_version: s("2.0"),
            package_base: s(name),
        }
    }

    #[test]
    fn pkgbuild_result_clears_install_review() {
        let mut transaction = new_transaction(InstallKind::Install);
        transaction.model.install_review = Some(InstallReview::new(vec![ignore_question("glibc")]));

        transaction.update(TransactionMessage::PkgbuildResult(Ok(vec![])));

        assert!(transaction.model.install_review.is_none());
    }

    #[test]
    fn explored_routes_review_checkout_or_failure() {
        let mut transaction = new_transaction(InstallKind::Install);
        transaction.update(TransactionMessage::Explored(Ok(initial_run(vec![
            ignore_question("glibc"),
        ]))));
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.checkout.is_none());
        assert_eq!(transaction.model.summary, Some(summary()));

        let mut transaction = new_transaction(InstallKind::Install);
        transaction.update(TransactionMessage::Explored(Ok(initial_run(Vec::new()))));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));
        assert!(transaction.model.pending_approvals.is_none());
    }

    #[test]
    fn approve_seals_and_waits_for_revalidation() {
        let mut transaction = new_transaction(InstallKind::Install);
        transaction.model.install_review = Some(InstallReview::new(vec![ignore_question("glibc")]));
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
    fn revalidation_diverges_then_converges_to_launch() {
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
        assert!(
            rebuilt
                .highlighted
                .contains(&ignore_question("nvidia").key())
        );

        let mut transaction = approving_transaction(Some(s("sealed-payload")), 1);
        transaction.update(TransactionMessage::Explored(Ok(settled_run(
            vec![ignore_question("glibc")],
            vec![ignore_answer("glibc")],
        ))));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Done(ChildOutcome::Failed(_))
        ));
        assert!(transaction.model.pending_approvals.is_none());
    }

    #[test]
    fn diverged_with_summary_only_drift_still_rebuilds() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 0);
        let mut varied = summary();
        varied.packages[0].new_version = s("2.0");
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Revalidation,
            vec![ignore_question("glibc")],
            vec![ignore_answer("glibc")],
            varied,
            Vec::new(),
        ))));
        assert_eq!(transaction.model.revalidations, 1);
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.pending_approvals.is_none());
    }

    #[test]
    fn unstable_notice_survives_restart_and_clears_stash() {
        let mut transaction = approving_transaction(Some(s("sealed-payload")), 2);
        transaction.update(TransactionMessage::Explored(Ok(drifted_run())));
        assert_eq!(transaction.model.revalidations, 0);
        assert!(transaction.model.pending_approvals.is_none());
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );

        transaction.update(TransactionMessage::Explored(Ok(initial_run(vec![
            ignore_question("glibc"),
        ]))));
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("review did not stabilize")
        );
        assert!(transaction.model.install_review.is_some());
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
        let review = transaction
            .model
            .install_review
            .as_ref()
            .expect("review kept");
        assert!(!review.approving);
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
        let review = transaction
            .model
            .install_review
            .as_ref()
            .expect("review kept");
        assert!(!review.approving);
    }

    #[test]
    fn sysupgrade_routes_by_questions_summary_and_aur() {
        let mut transaction = new_transaction(InstallKind::Upgrade);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            Vec::new(),
            Vec::new(),
            TransactionSummary::default(),
            Vec::new(),
        ))));
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Done(ChildOutcome::Success)
        ));
        assert_eq!(
            transaction.model.review_notice.as_deref(),
            Some("System is up to date")
        );

        let mut transaction = new_transaction(InstallKind::Upgrade);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            Vec::new(),
            Vec::new(),
            TransactionSummary::default(),
            vec![aur_candidate("yay")],
        ))));
        assert_eq!(transaction.model.aur_names, vec![s("yay")]);
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));

        let mut transaction = new_transaction(InstallKind::Upgrade);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            Vec::new(),
            Vec::new(),
            summary(),
            vec![aur_candidate("yay")],
        ))));
        assert_eq!(transaction.model.summary, Some(summary()));
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));

        let mut transaction = new_transaction(InstallKind::Upgrade);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            vec![ignore_question("glibc")],
            Vec::new(),
            summary(),
            vec![aur_candidate("yay")],
        ))));
        assert_eq!(transaction.model.aur_names, vec![s("yay")]);
        assert!(transaction.model.install_review.is_some());
        assert!(transaction.model.checkout.is_none());
    }

    #[test]
    fn sysupgrade_converged_revalidation_adopts_latest_aur_candidates() {
        let approved = |aur: Vec<_>| {
            let mut transaction = new_transaction(InstallKind::Upgrade);
            transaction.update(TransactionMessage::Explored(Ok(run(
                ReviewOrigin::Initial,
                vec![ignore_question("glibc")],
                Vec::new(),
                summary(),
                aur,
            ))));
            transaction.update(TransactionMessage::ApproveReview);
            transaction
        };

        let mut transaction = approved(vec![aur_candidate("yay")]);
        transaction.update(TransactionMessage::Explored(Ok(settled_run(
            vec![ignore_question("glibc")],
            vec![ignore_answer("glibc")],
        ))));
        assert!(transaction.model.aur_names.is_empty());
        assert!(transaction.model.install_review.is_none());
        assert!(transaction.model.checkout.is_none());
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));

        let mut transaction = approved(vec![aur_candidate("yay")]);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Revalidation,
            vec![ignore_question("glibc")],
            vec![ignore_answer("glibc")],
            summary(),
            vec![aur_candidate("paru")],
        ))));
        assert_eq!(transaction.model.aur_names, vec![s("paru")]);
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

    #[test]
    fn remove_routes_by_questions_and_gates_confirm_on_holds() {
        let mut transaction = new_transaction(InstallKind::Remove);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            Vec::new(),
            Vec::new(),
            removal_summary(),
            Vec::new(),
        ))));
        assert!(transaction.model.install_review.is_none());
        let checkout = transaction.model.checkout.as_ref().expect("checkout shown");
        assert_eq!(checkout.summary, removal_summary());

        let mut transaction = new_transaction(InstallKind::Remove);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            remove_questions(),
            Vec::new(),
            removal_summary(),
            Vec::new(),
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
        let mut transaction = new_transaction(InstallKind::Remove);
        transaction.update(TransactionMessage::Explored(Ok(run(
            ReviewOrigin::Initial,
            remove_questions(),
            Vec::new(),
            removal_summary(),
            Vec::new(),
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
            Answer::HoldPkgs { names, proceed: true }
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
        assert!(
            pakajo::dispatch::seal::decode_seal(&carried)
                .expect("decodes")
                .proceed
        );
        transaction.update(TransactionMessage::ApproveCheckout);
        assert!(matches!(
            transaction.model.status,
            TransactionStatus::Running
        ));
    }
}
