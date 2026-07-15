use crate::{
    aur::AurClient,
    install::{ChildOutcome, InstallProgress, StreamItem},
    install_log_overlay::InstallLogOverlay,
    package::{Package, PackageSource, is_installed},
    package_detail::PackageDetail,
    pacman::{find_pkg, init_alpm},
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider},
    search_view::{SearchState, SearchView, centered},
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

enum DetailPane {
    None,
    Loading,
    Ready(Entity<PackageDetail>),
    Error(String),
}

pub struct PakajoRoot {
    pub alpm_handle: Alpm,
    pub aur_client: Arc<AurClient>,
    pub install_progress: InstallProgress,
    search_input: Entity<InputState>,
    repo_index: Arc<RepoSearchIndex>,
    _subscriptions: Vec<Subscription>,
    search_seq: u64,
    search_state: SearchState,
    aur_error: Option<String>,
    detail: DetailPane,
    detail_seq: u64,
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
        let repo_index = Arc::new(RepoSearchIndex::from_alpm(&alpm_handle));
        let aur_client = Arc::new(aur_client);
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
        Self {
            alpm_handle,
            aur_client,
            install_progress: InstallProgress::Idle,
            search_input,
            repo_index,
            _subscriptions: vec![subscription],
            search_seq: 0,
            search_state: SearchState::Idle,
            aur_error: None,
            detail: DetailPane::None,
            detail_seq: 0,
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

        let repo_index = self.repo_index.clone();
        let aur_client = self.aur_client.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;

            let still_valid = this
                .update(cx, |this, _cx| this.search_seq == seq)
                .unwrap_or(false);
            if !still_valid {
                return;
            }

            let outcome = cx
                .background_executor()
                .spawn(async move { execute_search_for(&repo_index, &aur_client, &text) })
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
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let seq = self.detail_seq;
        self.search_view.set_selected_index(index);
        self.detail = DetailPane::Loading;
        cx.notify();

        match source {
            PackageSource::Repo => {
                if let Some(pkg) = find_pkg(&self.alpm_handle, &name) {
                    self.set_detail(Package::from(pkg), cx);
                } else {
                    self.detail = DetailPane::Error(format!("package not found: {name}"));
                    cx.notify();
                }
            }
            PackageSource::Aur => {
                let aur = self.aur_client.clone();
                cx.spawn(async move |this, cx| {
                    let name_for_info = name.clone();
                    let info = cx
                        .background_executor()
                        .spawn(async move { aur.info(&name_for_info) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.detail_seq != seq {
                            return;
                        }
                        match info {
                            Ok(Some(a)) => this.set_detail(Package::from(a), cx),
                            Ok(None) => {
                                this.detail =
                                    DetailPane::Error(format!("package not found: {name}"));
                                cx.notify();
                            }
                            Err(e) => {
                                this.detail = DetailPane::Error(search::friendly_search_error(&e));
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
        }
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

    fn set_detail(&mut self, pkg: Package, cx: &mut Context<Self>) {
        let installed = is_installed(&self.alpm_handle, &pkg.name);
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
        let name = match &self.detail {
            DetailPane::Ready(entity) => entity.read(cx).pkg.name.clone(),
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

        std::thread::spawn(move || crate::install::run_install_process(exe, name, tx));

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
                eprintln!("{ev:?}");
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
        if let Ok(config) = pacmanconf::Config::new()
            && let Ok(handle) = init_alpm(&config)
        {
            self.alpm_handle = handle;
        }

        if let DetailPane::Ready(entity) = &self.detail {
            let pkg_name = entity.read(cx).pkg.name.clone();
            let installed = is_installed(&self.alpm_handle, &pkg_name);
            entity.update(cx, |detail, cx| {
                detail.installed = installed;
                cx.notify();
            });
        }
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

fn execute_search_for(
    repo_index: &Arc<RepoSearchIndex>,
    aur_client: &Arc<AurClient>,
    text: &str,
) -> search::SearchOutcome {
    let repo_provider = RepoSearchProvider::new(repo_index.clone());
    let aur_provider = AurSearchProvider::new(aur_client.clone());
    search::execute_search(&repo_provider, &aur_provider, text)
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some("PakajoSearch")),
        KeyBinding::new("down", SelectDown, Some("PakajoSearch")),
    ]);
}
