use crate::{
    aur::AurClient,
    events::InstallEvent,
    install::InstallProgress,
    install_page::InstallPage,
    install_review_dialog::{self, InstallReviewDialog},
    package::PackageSource,
    package_detail::{DetailIntent, PackageDetail},
    pkgbuild::PkgbuildDiff,
    pkgbuild_review_dialog::PkgbuildReviewFlow,
    question::QuestionSet,
    search_view::{SearchView, centered},
    session::{DetailData, InstallKind, PakajoSession, SearchState, SessionEvent, UpdatesState},
    updates_view::UpdatesView,
};
use alpm::Alpm;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Root, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    progress::Progress,
    spinner::Spinner,
};
use std::sync::Arc;
use std::time::Duration;

actions!(pakajo, [SelectUp, SelectDown]);

enum DetailPane {
    None,
    Loading,
    Ready(Entity<PackageDetail>),
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Main,
    Install,
    Updates,
}

pub struct PakajoRoot {
    session: Entity<PakajoSession>,
    pub install_progress: InstallProgress,
    install_kind: InstallKind,
    status_overall: f32,
    status_indeterminate: bool,
    search_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    detail: DetailPane,
    detail_subscription: Option<Subscription>,
    search_view: SearchView,
    updates_view: UpdatesView,
    page: Page,
    install_page: Option<Entity<InstallPage>>,
    review_flow: PkgbuildReviewFlow,
    updates_flash: bool,
    flash_generation: u64,
    last_seen_count: u32,
}

impl PakajoRoot {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        alpm_handle: Alpm,
        aur_client: AurClient,
    ) -> Self {
        let session = cx.new(|_cx| PakajoSession::new(alpm_handle, aur_client));
        session.update(cx, |s, cx| s.start_db_lock_watcher(cx));
        session.update(cx, |s, cx| s.start_updates_checker(cx));
        let input_window = &mut *window;
        let search_input =
            cx.new(|cx| InputState::new(input_window, cx).placeholder("Search packages…"));
        search_input.focus_handle(cx).focus(window, cx);
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
            |this, _session, ev: &SessionEvent, window, cx| match ev {
                SessionEvent::DetailUpdated => this.on_detail_updated(cx),
                SessionEvent::SearchUpdated => this.on_search_updated(cx),
                SessionEvent::InstallProgressChanged(progress) => {
                    this.on_install_progress_changed(progress.clone(), window, cx)
                }
                SessionEvent::InstallLog(ev) => this.on_install_log(ev.clone(), cx),
                SessionEvent::InstallLogsOpened { kind, source, name } => {
                    this.on_install_logs_opened(kind.clone(), *source, name.clone(), window, cx)
                }
                SessionEvent::ReviewRequired { qs, name } => {
                    this.on_review_required(qs.clone(), name.clone(), window, cx)
                }
                SessionEvent::PkgbuildReviewRequired { diffs } => {
                    this.on_pkgbuild_review_required(diffs.clone(), window, cx)
                }
                SessionEvent::UpdatesAvailable(n) => {
                    let increased = *n > this.last_seen_count;
                    this.last_seen_count = *n;
                    if increased && *n > 0 {
                        this.updates_flash = true;
                        this.flash_generation = this.flash_generation.wrapping_add(1);
                        let flash_gen = this.flash_generation;
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(700))
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                if this.flash_generation == flash_gen {
                                    this.updates_flash = false;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                    cx.notify();
                }
            },
        );

        Self {
            session,
            install_progress: InstallProgress::Idle,
            install_kind: InstallKind::Install,
            status_overall: 0.0,
            status_indeterminate: true,
            search_input,
            _subscriptions: vec![subscription, session_subscription],
            detail: DetailPane::None,
            detail_subscription: None,
            search_view: SearchView::new(),
            updates_view: UpdatesView::new(),
            page: Page::Main,
            install_page: None,
            review_flow: PkgbuildReviewFlow::new(),
            updates_flash: false,
            flash_generation: 0,
            last_seen_count: 0,
        }
    }

    fn on_search_change(&mut self, cx: &mut Context<Self>) {
        let text = self.search_input.read(cx).value().to_string();
        self.session
            .update(cx, |session, cx| session.on_search_change(text, cx));
    }

    fn select_by_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.select_by_index(index, cx));
    }

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.session
            .update(cx, |session, cx| session.select_delta(-1, cx));
    }

    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.session
            .update(cx, |session, cx| session.select_delta(1, cx));
    }

    fn on_search_updated(&mut self, cx: &mut Context<Self>) {
        let index = self.session.read(cx).selected_index.unwrap_or(0);
        self.search_view.scroll_to(index);
        cx.notify();
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
                    let install_progress = self.install_progress.clone();
                    let entity = cx.new(|_| PackageDetail {
                        pkg,
                        installed,
                        active_tooltip: None,
                        install_progress,
                    });
                    self.detail_subscription = Some(cx.subscribe(&entity, Self::on_detail_intent));
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

    fn on_install_progress_changed(
        &mut self,
        progress: InstallProgress,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let DetailPane::Ready(entity) = &self.detail {
            entity.update(cx, |detail, cx| {
                detail.install_progress = progress.clone();
                cx.notify();
            });
        }
        if let Some(page) = &self.install_page {
            page.update(cx, |install_page, cx| {
                install_page.status = progress.clone();
                cx.notify();
            });
        }

        let was_conflict = matches!(self.install_progress, InstallProgress::ConflictReview(_));
        let is_pkgbuild_review = matches!(progress, InstallProgress::PkgbuildReview);

        if is_pkgbuild_review && !self.review_flow.is_active() {
            if was_conflict {
                window.close_dialog(cx);
            }

            let root_for_complete = cx.weak_entity();
            let on_complete = Box::new(move |window: &mut Window, cx: &mut App| {
                let Some(root) = root_for_complete.upgrade() else {
                    return;
                };
                root.update(cx, |this, cx| {
                    this.session
                        .update(cx, |s, cx| s.confirm_pkgbuild_review(cx));
                    this.review_flow.end(window, cx);
                });
            });

            let root_for_cancel = cx.weak_entity();
            let on_cancel = std::sync::Arc::new(move |window: &mut Window, cx: &mut App| {
                let Some(root) = root_for_cancel.upgrade() else {
                    return;
                };
                root.update(cx, |this, cx| {
                    this.session
                        .update(cx, |s, cx| s.cancel_pkgbuild_review(cx));
                    this.review_flow.end(window, cx);
                });
            })
                as std::sync::Arc<dyn Fn(&mut Window, &mut App) + 'static>;

            self.review_flow.begin(on_complete, on_cancel, window, cx);
        }

        if self.review_flow.is_active() && !matches!(progress, InstallProgress::PkgbuildReview) {
            self.review_flow.end(window, cx);
        }

        self.install_progress = progress;
        cx.notify();
    }

    fn on_install_log(&mut self, ev: InstallEvent, cx: &mut Context<Self>) {
        if let Some(page) = &self.install_page {
            page.update(cx, |install_page, cx| {
                install_page.handle_event(ev, cx);
            });
            let (overall, indeterminate) = {
                let pg = page.read(cx);
                (pg.overall, pg.indeterminate)
            };
            if (overall, indeterminate) != (self.status_overall, self.status_indeterminate) {
                self.status_overall = overall;
                self.status_indeterminate = indeterminate;
                cx.notify();
            }
        }
    }

    fn on_detail_intent(
        &mut self,
        _: Entity<PackageDetail>,
        intent: &DetailIntent,
        cx: &mut Context<Self>,
    ) {
        match intent {
            DetailIntent::Install => self.session.update(cx, |s, cx| s.start_install(cx)),
            DetailIntent::Remove => self.session.update(cx, |s, cx| s.start_remove(cx)),
        }
    }

    fn on_review_required(
        &mut self,
        qs: QuestionSet,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_for_title = name;

        let root_for_approve = cx.weak_entity();
        let on_approve = Box::new(
            move |approvals: crate::question::Approvals, _window: &mut Window, cx: &mut App| {
                let Some(root) = root_for_approve.upgrade() else {
                    return;
                };
                root.update(cx, |this, cx| {
                    this.session
                        .update(cx, |s, cx| s.confirm_install(approvals, cx));
                });
            },
        );

        let root_for_cancel = cx.weak_entity();
        let on_cancel = std::sync::Arc::new(move |_window: &mut Window, cx: &mut App| {
            if let Some(root) = root_for_cancel.upgrade() {
                root.update(cx, |this, cx| {
                    this.session.update(cx, |s, cx| s.cancel_install(cx));
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

    fn on_pkgbuild_review_required(
        &mut self,
        diffs: Vec<PkgbuildDiff>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.review_flow.show_review(diffs, cx);
    }

    fn on_install_logs_opened(
        &mut self,
        kind: InstallKind,
        source: PackageSource,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.install_kind = kind;
        let root_entity = cx.entity();
        let on_back: Arc<dyn Fn(&mut Window, &mut App) + 'static> = Arc::new(move |_window, cx| {
            root_entity.update(cx, |root, cx| {
                root.page = Page::Main;
                cx.notify();
            });
        });
        let current_progress = self.install_progress.clone();
        let page = cx.new(|_| {
            let mut page = InstallPage::new(kind, source, name, on_back);
            page.status = current_progress;
            page
        });
        self.install_page = Some(page);
        self.page = Page::Install;
        self.status_overall = 0.0;
        self.status_indeterminate = true;
        window.close_all_dialogs(cx);
    }

    fn dismiss_install(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| s.reset_install(cx));
        self.install_progress = InstallProgress::Idle;
        self.install_page = None;
        self.page = Page::Main;
        self.status_overall = 0.0;
        self.status_indeterminate = true;
        cx.notify();
    }

    fn render_install_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (present, past) = match self.install_kind {
            InstallKind::Install => ("Installing", "Install"),
            InstallKind::Remove => ("Removing", "Remove"),
        };
        let (label, color): (String, Hsla) = match &self.install_progress {
            InstallProgress::Running => (format!("{present}…"), cx.theme().muted_foreground),
            InstallProgress::Failed(_) => (format!("{past} failed"), cx.theme().danger),
            InstallProgress::Completed => (format!("{past} complete"), cx.theme().green),
            InstallProgress::Cancelled => ("Cancelled".to_string(), cx.theme().muted_foreground),
            InstallProgress::Idle
            | InstallProgress::ConflictReview(_)
            | InstallProgress::PkgbuildReview => return None,
        };
        let show_bar = matches!(self.install_progress, InstallProgress::Running);
        let terminal = matches!(
            self.install_progress,
            InstallProgress::Completed | InstallProgress::Failed(_) | InstallProgress::Cancelled
        );
        Some(
            div()
                .id("install-status-bar")
                .h_flex()
                .items_center()
                .gap_2()
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(cx.theme().title_bar)
                .child(
                    div()
                        .id("install-status-label")
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(div().text_sm().child(label))
                        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                            if this.install_page.is_some() {
                                this.page = Page::Install;
                                cx.notify();
                            }
                        })),
                )
                .children(show_bar.then(|| {
                    if self.status_indeterminate {
                        Progress::new("install-status").loading(true).w(px(200.))
                    } else {
                        Progress::new("install-status")
                            .value(self.status_overall)
                            .w(px(200.))
                    }
                }))
                .children(terminal.then(|| {
                    Button::new("install-dismiss")
                        .icon(IconName::Close)
                        .ghost()
                        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.dismiss_install(cx);
                        }))
                }))
                .into_any_element(),
        )
    }

    fn render_updates_badge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let count = session.pending_count;
        let theme = cx.theme();

        let flash_bg = Hsla {
            l: (theme.primary.l + 0.12).min(1.0),
            ..theme.primary
        };
        let primary_bg = if self.updates_flash {
            flash_bg
        } else {
            theme.primary
        };

        let (bg, fg, extra) = match (&session.updates_state, count) {
            (UpdatesState::Loading, _) => (
                theme.accent,
                theme.muted_foreground,
                Spinner::new().into_any_element(),
            ),
            (UpdatesState::Error(_), _) => (
                theme.danger,
                theme.danger_foreground,
                div().into_any_element(),
            ),
            (UpdatesState::Idle, c) if c > 0 => (
                primary_bg,
                theme.primary_foreground,
                c.to_string().into_any_element(),
            ),
            _ => (
                theme.accent,
                theme.muted_foreground,
                div().into_any_element(),
            ),
        };

        div()
            .id("updates-badge")
            .h_flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.))
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .text_xs()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.page = Page::Updates;
                cx.notify();
            }))
            .child(Icon::new(IconName::ArrowDown))
            .child(extra)
    }

    fn render_updates_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let updates = session.pending_updates.clone();
        let state = session.updates_state.clone();
        let aur_error = session.updates_aur_error.clone();

        div()
            .v_flex()
            .size_full()
            .gap_2()
            .child(
                Button::new("updates-back")
                    .ghost()
                    .icon(IconName::ArrowLeft)
                    .label("Back")
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.page = Page::Main;
                        cx.notify();
                    })),
            )
            .child(
                self.updates_view
                    .render(&updates, state, aur_error.as_deref(), cx),
            )
    }
}

impl Render for PakajoRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let on_select: Arc<dyn Fn(usize, &mut App)> = Arc::new(move |index, cx| {
            entity.update(cx, |root, cx| root.select_by_index(index, cx));
        });

        let is_install = self.page == Page::Install && self.install_page.is_some();

        let body: AnyElement = match (&self.page, &self.install_page) {
            (Page::Install, Some(page)) => div()
                .flex_1()
                .size_full()
                .min_h_0()
                .child(page.clone())
                .into_any_element(),
            (Page::Updates, _) => self.render_updates_page(cx).into_any_element(),
            _ => {
                let session = self.session.read(cx);
                if session.results.is_empty() {
                    let status = match session.search_state {
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
                        .child(self.search_view.render(
                            &session.results,
                            session.selected_index,
                            session.search_state,
                            on_select,
                            cx,
                        ))
                        .child(detail_pane)
                        .into_any_element()
                }
            }
        };

        let shell = div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .font_family("Inter")
            .id("pakajo-root")
            .key_context("PakajoSearch")
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down));

        let shell = if is_install {
            shell.child(body)
        } else {
            shell
                .child(
                    h_flex()
                        .gap_2()
                        .child(Input::new(&self.search_input).rounded_none().flex_1())
                        .child(self.render_updates_badge(cx)),
                )
                .child(body)
        };

        shell
            .children(self.render_install_status(cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some("PakajoSearch")),
        KeyBinding::new("down", SelectDown, Some("PakajoSearch")),
    ]);
}
