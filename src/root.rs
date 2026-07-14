use crate::{
    aur::AurClient,
    package::is_installed,
    package_detail::PackageDetail,
    pacman::init_alpm,
    search::{self, AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult},
};
use alpm::Alpm;
use futures::StreamExt as _;
use gpui::*;
use gpui_component::{
    input::{Input, InputEvent, InputState},
    ActiveTheme as _,
    StyledExt as _,
};
use std::{
    io::{self, BufRead},
    process::{Command, ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
pub enum InstallProgress {
    Idle,
    Running,
    Failed(String),
}

#[derive(Clone, Debug)]
enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

enum StreamItem {
    Event(crate::events::InstallEvent),
    Done(ChildOutcome),
}

#[derive(Clone, Copy, PartialEq)]
enum SearchState {
    Idle,
    Searching,
    Done,
}

pub struct PakajoRoot {
    pub alpm_handle: Alpm,
    pub aur_client: Arc<AurClient>,
    pub target_package: String,
    pub package_detail: Option<Entity<PackageDetail>>,
    pub install_progress: InstallProgress,
    search_input: Entity<InputState>,
    repo_index: Arc<RepoSearchIndex>,
    _subscriptions: Vec<Subscription>,
    search_seq: u64,
    results: Vec<SearchResult>,
    search_state: SearchState,
    aur_error: Option<String>,
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
        let search_input = cx.new(|cx| {
            InputState::new(input_window, cx).placeholder("Search packages…")
        });
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
            target_package: String::new(),
            package_detail: None,
            install_progress: InstallProgress::Idle,
            search_input,
            repo_index,
            _subscriptions: vec![subscription],
            search_seq: 0,
            results: Vec::new(),
            search_state: SearchState::Idle,
            aur_error: None,
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
                for result in &this.results {
                    eprintln!(
                        "  {} {} [{}] {}",
                        result.name,
                        result.version,
                        result.repo.as_deref().unwrap_or("-"),
                        result.description.as_deref().unwrap_or("-"),
                    );
                }
                if let Some(err) = &this.aur_error {
                    eprintln!("  aur: {err}");
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_progress(&mut self, progress: InstallProgress, cx: &mut Context<Self>) {
        if let Some(detail) = &self.package_detail {
            detail.update(cx, |detail, cx| {
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
        self.set_progress(InstallProgress::Running, cx);

        let name = self.target_package.clone();
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

        let (mut tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);

        std::thread::spawn(move || {
            let mut send_event = |mut item: StreamItem| loop {
                match tx.try_send(item) {
                    Ok(()) => return,
                    Err(err) => {
                        if err.is_disconnected() {
                            return;
                        }
                        item = err.into_inner();
                        std::thread::yield_now();
                    }
                }
            };

            let outcome = match Command::new(&exe)
                .arg("install")
                .arg("--json")
                .arg(&name)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
            {
                Err(error) if error.kind() == io::ErrorKind::NotFound => ChildOutcome::NotFound,
                Err(error) => ChildOutcome::Failed(error.to_string()),
                Ok(mut child) => {
                    let stdout = child.stdout.take().expect("piped");
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(l) => {
                                if let Ok(ev) =
                                    serde_json::from_str::<crate::events::InstallEvent>(&l)
                                {
                                    send_event(StreamItem::Event(ev));
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    map_outcome(child.wait())
                }
            };
            send_event(StreamItem::Done(outcome));
        });

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

        let installed = if let Some(detail) = &self.package_detail {
            let pkg_name = detail.read(cx).pkg.name.clone();
            is_installed(&self.alpm_handle, &pkg_name)
        } else {
            false
        };

        if let Some(detail) = &self.package_detail {
            detail.update(cx, |detail, cx| {
                detail.installed = installed;
                cx.notify();
            });
        }
    }
}

impl Render for PakajoRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = match self.search_state {
            SearchState::Idle => "Search for packages to get started".to_string(),
            SearchState::Searching => "Searching…".to_string(),
            SearchState::Done => {
                if self.results.is_empty() {
                    "No packages found".to_string()
                } else {
                    format!("{} result(s)", self.results.len())
                }
            }
        };

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .font_family("Inter")
            .child(Input::new(&self.search_input))
            .child(
                div()
                    .flex_1()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
    }
}

fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
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
