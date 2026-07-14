use crate::{
    aur::AurClient,
    install::{ChildOutcome, InstallProgress, StreamItem},
    package::{Package, PackageSource, is_installed},
    package_detail::PackageDetail,
    pacman::{find_pkg, init_alpm},
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult},
};
use alpm::Alpm;
use futures::StreamExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, StyledExt as _,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, PartialEq)]
enum SearchState {
    Idle,
    Searching,
    Done,
}

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
    results: Vec<SearchResult>,
    search_state: SearchState,
    aur_error: Option<String>,
    detail: DetailPane,
    selected: Option<String>,
    detail_seq: u64,
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
            results: Vec::new(),
            search_state: SearchState::Idle,
            aur_error: None,
            detail: DetailPane::None,
            selected: None,
            detail_seq: 0,
        }
    }

    fn on_search_change(&mut self, cx: &mut Context<Self>) {
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        let text = self.search_input.read(cx).value().to_string();

        if text.trim().is_empty() {
            self.results.clear();
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
                this.results = outcome.results;
                this.aur_error = outcome.aur_error;
                this.search_state = SearchState::Done;
                if let Some(err) = &this.aur_error {
                    eprintln!("  aur: {err}");
                }
                let first = this.results.first().map(|r| (r.name.clone(), r.source));
                if let Some((name, source)) = first {
                    let unchanged =
                        matches!(this.detail, DetailPane::Ready(_) | DetailPane::Loading)
                            && this.selected.as_deref() == Some(name.as_str());
                    if !unchanged {
                        this.select(name, source, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, name: String, source: PackageSource, cx: &mut Context<Self>) {
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let seq = self.detail_seq;
        self.selected = Some(name.clone());
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

    pub fn start_install(&mut self, cx: &mut Context<Self>) {
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

impl PakajoRoot {
    fn result_row(
        &self,
        result: &SearchResult,
        entity: Entity<PakajoRoot>,
        cx: &App,
    ) -> impl IntoElement {
        let is_selected = self.selected.as_deref() == Some(result.name.as_str());
        let badge = result.repo.as_deref().unwrap_or("aur");
        let entity_for_click = entity.clone();
        let name_for_click = result.name.clone();
        let source_for_click = result.source;
        let accent = cx.theme().accent;
        let muted = cx.theme().muted_foreground;
        div()
            .id(result.name.clone())
            .v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .when(is_selected, |row| row.bg(accent.opacity(0.12)))
            .hover(|s| s.bg(accent.opacity(0.06)))
            .on_click(move |_, _, cx| {
                entity_for_click.update(cx, |this, cx| {
                    this.select(name_for_click.clone(), source_for_click, cx)
                });
            })
            .child(
                div()
                    .h_flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().font_semibold().child(result.name.clone()))
                    .child(
                        div()
                            .text_color(muted)
                            .text_size(rems(0.75))
                            .child(badge.to_string()),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_color(muted)
                            .text_size(rems(0.75))
                            .child(result.version.clone()),
                    ),
            )
            .children(result.description.clone().map(|d| {
                div()
                    .w_full()
                    .truncate()
                    .text_color(muted)
                    .text_size(rems(0.8))
                    .child(d)
            }))
    }
}

impl Render for PakajoRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        let body = if self.results.is_empty() {
            let status = match self.search_state {
                SearchState::Idle => "Search for packages to get started".to_string(),
                SearchState::Searching => "Searching…".to_string(),
                SearchState::Done => "No packages found".to_string(),
            };
            div()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(status)
                .into_any_element()
        } else {
            let header = if matches!(self.search_state, SearchState::Searching) {
                "Searching…".to_string()
            } else {
                format!("{} result(s)", self.results.len())
            };

            let mut rows: Vec<AnyElement> = Vec::with_capacity(self.results.len());
            for result in &self.results {
                rows.push(
                    self.result_row(result, entity.clone(), cx)
                        .into_any_element(),
                );
            }

            let list_pane = div()
                .w(rems(24.))
                .flex_grow_0()
                .h_full()
                .v_flex()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(cx.theme().muted_foreground)
                        .child(header),
                )
                .child(
                    div()
                        .id("results")
                        .flex_1()
                        .overflow_y_scroll()
                        .v_flex()
                        .children(rows),
                );

            let detail_pane = match &self.detail {
                DetailPane::None => div()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a package")
                    .into_any_element(),
                DetailPane::Loading => div()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new())
                    .into_any_element(),
                DetailPane::Error(message) => div()
                    .flex_1()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().danger_foreground)
                    .child(message.clone())
                    .into_any_element(),
                DetailPane::Ready(detail_entity) => div()
                    .flex_1()
                    .h_full()
                    .child(detail_entity.clone())
                    .into_any_element(),
            };

            div()
                .flex_1()
                .size_full()
                .h_flex()
                .gap_4()
                .child(list_pane)
                .child(detail_pane)
                .into_any_element()
        };

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .font_family("Inter")
            .child(Input::new(&self.search_input))
            .child(body)
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
