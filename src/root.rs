use crate::{
    aur::AurClient,
    install::{ChildOutcome, InstallProgress, StreamItem},
    install_log_overlay::InstallLogOverlay,
    install_review_dialog::{self, InstallReviewDialog},
    package::{PackageSource, installed_names, is_installed},
    package_detail::PackageDetail,
    pacman::init_alpm,
    question::QuestionSet,
    search_view::{SearchState, SearchView, centered},
    session::{DetailData, PakajoSession, SessionEvent},
};
use alpm::Alpm;
use futures::StreamExt as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Root, StyledExt as _, WindowExt as _,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
};
use std::{sync::Arc, time::Duration};

actions!(pakajo, [SelectUp, SelectDown]);

const LIVE_DEBOUNCE: Duration = Duration::from_millis(300);

enum DetailPane {
    None,
    Loading,
    Ready(Entity<PackageDetail>),
    Error(String),
}

pub struct PakajoRoot {
    session: Entity<PakajoSession>,
    pub install_progress: InstallProgress,
    search_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    search_seq: u64,
    search_state: SearchState,
    aur_error: Option<String>,
    detail: DetailPane,
    search_view: SearchView,
    install_log_view: Option<Entity<InstallLogOverlay>>,
}

impl PakajoRoot {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        alpm_handle: Alpm,
        aur_client: AurClient,
    ) -> Self {
        let session = cx.new(|_cx| PakajoSession::new(alpm_handle, aur_client));
        let input_window = &mut *window;
        let search_input =
            cx.new(|cx| InputState::new(input_window, cx).placeholder("Search packages…"));
        let sub_window = &mut *window;
        let subscription = cx.subscribe_in(
            &search_input,
            sub_window,
            |this, _state, ev: &InputEvent, _window, cx| match ev {
                InputEvent::Change => this.on_search_change(cx),
                _ => {}
            },
        );

        let session_window = &mut *window;
        let session_subscription = cx.subscribe_in(
            &session,
            session_window,
            |this, _session, ev: &SessionEvent, _window, cx| match ev {
                SessionEvent::DetailUpdated => this.on_detail_updated(cx),
            },
        );

        Self {
            session,
            install_progress: InstallProgress::Idle,
            search_input,
            _subscriptions: vec![subscription, session_subscription],
            search_seq: 0,
            search_state: SearchState::Idle,
            aur_error: None,
            detail: DetailPane::None,
            search_view: SearchView::new(),
            install_log_view: None,
        }
    }

    fn on_search_change(&mut self, cx: &mut Context<Self>) {
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        let text = self.search_input.read(cx).value().to_string();

        if text.trim().is_empty() {
            self.search_view.clear();
            self.aur_error = None;
            self.search_state = SearchState::Idle;
            cx.notify();
            return;
        }

        self.search_state = SearchState::Searching;
        self.aur_error = None;
        cx.notify();

        let repo_index = self.session.read(cx).repo_index.clone();
        let aur_client = self.session.read(cx).aur_client.clone();
        let local_index = self.session.read(cx).local_index.clone();
        let installed = self.session.read(cx).installed_names.clone();
        let debounce = if self
            .session
            .read(cx)
            .local_index
            .as_ref()
            .is_some_and(|i| i.is_populated())
        {
            Duration::ZERO
        } else {
            LIVE_DEBOUNCE
        };
        cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }

            let still_valid = this
                .update(cx, |this, _cx| this.search_seq == seq)
                .unwrap_or(false);
            if !still_valid {
                return;
            }

            let outcome = cx
                .background_executor()
                .spawn(async move {
                    crate::session::execute_search_for(
                        local_index, &repo_index, &aur_client, installed, &text,
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.search_seq != seq {
                    return;
                }
                let prev_selected = this.search_view.selected_name().map(str::to_string);
                this.search_view.set_results(outcome.results);
                this.aur_error = outcome.aur_error;
                this.search_state = SearchState::Done;
                if let Some(err) = &this.aur_error {
                    eprintln!("  aur: {err}");
                }
                let first_name = this.search_view.first_result().map(|r| r.name.clone());
                if let Some(name) = first_name {
                    let unchanged =
                        matches!(this.detail, DetailPane::Ready(_) | DetailPane::Loading)
                            && prev_selected.as_deref() == Some(name.as_str());
                    if !unchanged {
                        this.select_by_index(0, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_by_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(result) = self.search_view.result_at(index) else {
            return;
        };
        let name = result.name.clone();
        let source = result.source;
        self.search_view.set_selected_index(index);
        self.session
            .update(cx, |session, cx| session.load_detail(name, source, cx));
    }

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if let Some(ix) = self.search_view.move_cursor(-1) {
            self.select_by_index(ix, cx);
        }
    }

    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if let Some(ix) = self.search_view.move_cursor(1) {
            self.select_by_index(ix, cx);
        }
    }

    fn on_detail_updated(&mut self, cx: &mut Context<Self>) {
        let incoming = self.session.read(cx).detail.clone();
        match incoming {
            DetailData::None => {
                self.detail = DetailPane::None;
                cx.notify();
            }
            DetailData::Loading => {
                self.detail = DetailPane::Loading;
                cx.notify();
            }
            DetailData::Error(message) => {
                self.detail = DetailPane::Error(message);
                cx.notify();
            }
            DetailData::Ready { pkg, installed } => {
                let rebuild = match &self.detail {
                    DetailPane::Ready(entity) => entity.read(cx).pkg.name != pkg.name,
                    _ => true,
                };
                if rebuild {
                    let root = cx.weak_entity();
                    let install_progress = self.install_progress.clone();
                    let entity = cx.new(|_| PackageDetail {
                        pkg,
                        installed,
                        active_tooltip: None,
                        root,
                        install_progress,
                    });
                    self.detail = DetailPane::Ready(entity);
                    cx.notify();
                } else if let DetailPane::Ready(entity) = &self.detail {
                    entity.update(cx, |detail, cx| {
                        detail.pkg = pkg;
                        detail.installed = installed;
                        cx.notify();
                    });
                }
            }
        }
    }

    fn set_progress(&mut self, progress: InstallProgress, cx: &mut Context<Self>) {
        if let DetailPane::Ready(entity) = &self.detail {
            entity.update(cx, |detail, cx| {
                detail.install_progress = progress.clone();
                cx.notify();
            });
        }
        self.install_progress = progress;
        cx.notify();
    }

    pub fn start_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        let (name, source) = match &self.detail {
            DetailPane::Ready(entity) => {
                let name = entity.read(cx).pkg.name.clone();
                let source = entity.read(cx).pkg.source;
                (name, source)
            }
            _ => return,
        };
        self.set_progress(InstallProgress::Running, cx);

        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };

        if matches!(source, PackageSource::Repo) {
            self.begin_install_subprocess(exe, name, None, window, cx);
            return;
        }

        let (mut dry_tx, mut dry_rx) =
            futures::channel::mpsc::channel::<anyhow::Result<crate::question::QuestionSet>>(1);
        let name_for_dry_run = name.clone();
        std::thread::spawn(move || {
            let result = crate::dry_run::dry_run_for_target(&name_for_dry_run);
            let _ = dry_tx.try_send(result);
        });

        cx.spawn_in(window, async move |this, cx| {
            let Some(result) = dry_rx.next().await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(qs)
                    if !qs.conflicts.is_empty()
                        || !qs.providers.is_empty()
                        || qs.had_unsupported_question =>
                {
                    this.open_install_review(qs, exe, name, window, cx);
                }
                Ok(_) => {
                    this.begin_install_subprocess(exe, name, None, window, cx);
                }
                Err(err) => {
                    eprintln!("dry-run failed, proceeding with install: {err:#}");
                    this.begin_install_subprocess(exe, name, None, window, cx);
                }
            });
        })
        .detach();
    }

    pub fn start_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        let name = match &self.detail {
            DetailPane::Ready(entity) => entity.read(cx).pkg.name.clone(),
            _ => return,
        };
        if !is_installed(&self.session.read(cx).alpm_handle, &name) {
            return;
        }
        self.set_progress(InstallProgress::Running, cx);

        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };

        self.begin_remove_subprocess(exe, name, window, cx);
    }

    fn open_install_review(
        &mut self,
        qs: QuestionSet,
        exe: std::path::PathBuf,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_progress(InstallProgress::ConflictReview(qs.clone()), cx);

        let root_for_approve = cx.weak_entity();
        let exe_for_approve = exe;
        let name_for_approve = name.clone();
        let name_for_title = name.clone();
        let on_approve = Box::new(
            move |approvals: crate::question::Approvals, window: &mut Window, cx: &mut App| {
                let Some(root) = root_for_approve.upgrade() else {
                    return;
                };
                root.update(cx, |this, cx| {
                    let b64 = match crate::question::encode_approvals(&approvals) {
                        Ok(b64) => b64,
                        Err(error) => {
                            this.set_progress(
                                InstallProgress::Failed(format!(
                                    "failed to encode approvals: {error}"
                                )),
                                cx,
                            );
                            return;
                        }
                    };
                    this.set_progress(InstallProgress::Running, cx);
                    this.begin_install_subprocess(
                        exe_for_approve,
                        name_for_approve,
                        Some(b64),
                        window,
                        cx,
                    );
                });
            },
        );

        let root_for_cancel = cx.weak_entity();
        let on_cancel = std::sync::Arc::new(move |_window: &mut Window, cx: &mut App| {
            if let Some(root) = root_for_cancel.upgrade() {
                root.update(cx, |this, cx| {
                    this.set_progress(InstallProgress::Idle, cx);
                });
            }
        }) as std::sync::Arc<dyn Fn(&mut Window, &mut App) + 'static>;

        let on_cancel_for_dialog = on_cancel.clone();
        let review = cx.new(|_| InstallReviewDialog::new(qs, on_approve, on_cancel));

        window.open_dialog(cx, move |dialog, _window, cx| {
            let title = install_review_dialog::build_title(&name_for_title, cx);
            let on_cancel_arc = on_cancel_for_dialog.clone();
            let review_for_content = review.clone();
            dialog
                .title(title)
                .w(px(560.))
                .close_button(true)
                .on_cancel(move |_, window, cx| {
                    on_cancel_arc(window, cx);
                    true
                })
                .content(move |content, _window, _cx| content.child(review_for_content.clone()))
        });
    }

    fn begin_install_subprocess(
        &mut self,
        exe: std::path::PathBuf,
        name: String,
        approvals_b64: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let log_view = cx.new(|_| InstallLogOverlay { logs: Vec::new() });
        self.install_log_view = Some(log_view.clone());
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title("Install logs")
                .w(px(720.))
                .child(log_view.clone())
        });

        let (tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);

        std::thread::spawn(move || {
            crate::install::run_install_process(exe, name, tx, approvals_b64)
        });

        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    fn begin_remove_subprocess(
        &mut self,
        exe: std::path::PathBuf,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let log_view = cx.new(|_| InstallLogOverlay { logs: Vec::new() });
        self.install_log_view = Some(log_view.clone());
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title("Install logs")
                .w(px(720.))
                .child(log_view.clone())
        });

        let (tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);

        std::thread::spawn(move || crate::remove::run_remove_process(exe, name, tx));

        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    fn handle_stream_item(&mut self, item: StreamItem, cx: &mut Context<Self>) {
        match item {
            StreamItem::Event(ev) => {
                if let Some(view) = &self.install_log_view {
                    view.update(cx, |overlay, cx| {
                        overlay.logs.push(format!("{ev:?}"));
                        cx.notify();
                    });
                }
            }
            StreamItem::Done(ChildOutcome::Success) => {
                self.refresh_after_install(cx);
                self.set_progress(InstallProgress::Idle, cx);
            }
            StreamItem::Done(ChildOutcome::Dismissed) => {
                self.set_progress(InstallProgress::Idle, cx);
            }
            StreamItem::Done(ChildOutcome::NotFound) => {
                self.set_progress(
                    InstallProgress::Failed("install child not found".into()),
                    cx,
                );
            }
            StreamItem::Done(ChildOutcome::Failed(message)) => {
                self.set_progress(InstallProgress::Failed(message), cx);
            }
        }
    }

    fn refresh_after_install(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            if let Ok(config) = pacmanconf::Config::new()
                && let Ok(handle) = init_alpm(&config)
            {
                s.alpm_handle = handle;
            }
            s.installed_names = Arc::new(installed_names(&s.alpm_handle));
            s.refresh_detail_installed(cx);
        });
    }
}

impl Render for PakajoRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let on_select: Arc<dyn Fn(usize, &mut App)> = Arc::new(move |index, cx| {
            entity.update(cx, |root, cx| root.select_by_index(index, cx));
        });

        let body = if self.search_view.is_empty() {
            let status = match self.search_state {
                SearchState::Idle => "Search for packages to get started".to_string(),
                SearchState::Searching => "Searching…".to_string(),
                SearchState::Done => "No packages found".to_string(),
            };
            centered()
                .text_color(cx.theme().muted_foreground)
                .child(status)
                .into_any_element()
        } else {
            let detail_pane = match &self.detail {
                DetailPane::None => centered()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a package")
                    .into_any_element(),
                DetailPane::Loading => centered().child(Spinner::new()).into_any_element(),
                DetailPane::Error(message) => centered()
                    .text_color(cx.theme().danger_foreground)
                    .child(message.clone())
                    .into_any_element(),
                DetailPane::Ready(detail_entity) => div()
                    .flex_1()
                    .size_full()
                    .child(detail_entity.clone())
                    .into_any_element(),
            };

            div()
                .flex_1()
                .size_full()
                .h_flex()
                .min_h_0()
                .gap_4()
                .child(self.search_view.render(self.search_state, on_select, cx))
                .child(detail_pane)
                .into_any_element()
        };

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .font_family("Inter")
            .id("pakajo-root")
            .key_context("PakajoSearch")
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .child(Input::new(&self.search_input).rounded_none())
            .child(body)
            .children(Root::render_dialog_layer(window, cx))
    }
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some("PakajoSearch")),
        KeyBinding::new("down", SelectDown, Some("PakajoSearch")),
    ]);
}
