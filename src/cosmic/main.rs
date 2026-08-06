mod background;
mod detail;
mod search;
mod transaction;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use cosmic::widget::{Column, Row, container, scrollable};
use cosmic::{
    Application, Element,
    app::{self, Core, Settings, Task},
    executor,
};
use pakajo::aur::AurClient;
use pakajo::cli;
use pakajo::local_index::LocalIndex;
use pakajo::pacman::init_alpm;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

use background::begin_aur_sync_in_background;
use detail::{DetailData, DetailMessage, detail_view};
use search::{SearchMessage, SearchState, results_list, search_bar, search_status_text};
use transaction::{Action, Transaction, TransactionMessage};

fn main() -> cosmic::iced::Result {
    let cli = cli::parse();
    cli::dispatch(cli);
    let settings = Settings::default().client_decorations(false);
    let flags = ();
    app::run::<PakajoApp>(settings, flags)
}

pub struct PakajoApp {
    core: Core,
    pub(crate) search_engine: Option<Arc<SearchEngine>>,
    pub(crate) local_index: Option<Arc<LocalIndex>>,
    pub(crate) alpm: Option<alpm::Alpm>,
    pub(crate) aur_client: Option<Arc<AurClient>>,
    pub(crate) installed_names: Arc<HashSet<String>>,
    pub(crate) group_index: Arc<Vec<(String, String)>>,
    pub(crate) query: String,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) search_state: SearchState,
    pub(crate) search_seq: u64,
    pub(crate) selected_index: Option<usize>,
    pub(crate) detail: DetailData,
    pub(crate) detail_seq: u64,
    pub(crate) transaction: Option<Transaction>,
    pub(crate) updates_state: UpdatesState,
    pub(crate) pending_updates: pakajo::updates::PendingUpdates,
    pub(crate) pending_count: u32,
    pub(crate) updates_aur_error: Option<String>,
}

impl Application for PakajoApp {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.pakajo.Pakajo";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let search_engine = LocalIndex::db_path()
            .ok()
            .and_then(|p| SearchEngine::new(p).ok())
            .map(Arc::new);
        let local_index = LocalIndex::db_path()
            .ok()
            .and_then(|p| {
                LocalIndex::open(&p)
                    .map_err(|e| {
                        eprintln!("local index unavailable, falling back to live search: {e:#}")
                    })
                    .ok()
            })
            .map(Arc::new);

        let (alpm, installed_names, group_index) = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(handle) => {
                let installed = Arc::new(pakajo::package::installed_names(&handle));
                let groups = Arc::new(pakajo::pacman::collect_group_index(&handle));
                (Some(handle), installed, groups)
            }
            Err(e) => {
                eprintln!("[pakajo] failed to snapshot installed packages and groups: {e:#}");
                (None, Arc::new(HashSet::new()), Arc::new(Vec::new()))
            }
        };

        let aur_client = Some(Arc::new(AurClient::new()));

        if let Some(index) = &local_index {
            begin_aur_sync_in_background(index.clone(), search_engine.clone());
        }

        let mut app = PakajoApp {
            core,
            search_engine,
            local_index,
            alpm,
            aur_client,
            installed_names,
            group_index,
            query: String::new(),
            results: Vec::new(),
            search_state: SearchState::Idle,
            search_seq: 0,
            selected_index: None,
            detail: DetailData::None,
            detail_seq: 0,
            transaction: None,
            updates_state: UpdatesState::Idle,
            pending_updates: pakajo::updates::PendingUpdates {
                repo: Vec::new(),
                aur: Vec::new(),
            },
            pending_count: 0,
            updates_aur_error: None,
        };
        let task = app.start_updates_check();
        (app, task)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Search(m) => self.handle_search(m),
            Message::Detail(m) => self.handle_detail(m),
            Message::Transaction(m) => self.handle_transaction(m),
            Message::Updates(m) => self.handle_updates(m),
        }
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        cosmic::iced::event::listen_with(|event, _status, _id| match event {
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key,
                ..
            }) => match key {
                cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::ArrowUp) => {
                    Some(Message::Search(SearchMessage::SelectDelta(-1)))
                }
                cosmic::iced::keyboard::Key::Named(
                    cosmic::iced::keyboard::key::Named::ArrowDown,
                ) => Some(Message::Search(SearchMessage::SelectDelta(1))),
                _ => None,
            },
            _ => None,
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if let Some(t) = self.transaction.as_ref() {
            if !t.is_checking() {
                return container(t.view())
                    .width(cosmic::iced::Length::Fill)
                    .height(cosmic::iced::Length::Fill)
                    .into();
            }
        }
        let checking = self.transaction.as_ref().map(|t| t.name());
        let content = Column::new()
            .spacing(12)
            .push(search_bar(&self.query))
            .push(search_status_text(self.search_state, self.results.len()))
            .push(
                Row::new()
                    .push(scrollable(results_list(&self.results, self.selected_index)).width(384.))
                    .push(detail_view(&self.detail, checking)),
            );

        container(content).into()
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        self.transaction.as_ref().and_then(|t| t.dialog())
    }
}

impl PakajoApp {
    fn handle_transaction(&mut self, message: TransactionMessage) -> Task<Message> {
        match message {
            TransactionMessage::StartInstall => {
                if self.transaction.as_ref().is_some_and(Transaction::is_active) {
                    return Task::none();
                }
                let (name, source) = match &self.detail {
                    DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source),
                    _ => return Task::none(),
                };
                let (txn, task) = Transaction::start(name, source);
                self.transaction = Some(txn);
                task
            }
            other => {
                let action = match self.transaction.as_mut() {
                    Some(t) => t.update(other),
                    None => Action::None,
                };
                match action {
                    Action::None => Task::none(),
                    Action::Run(task) => task,
                    Action::Finished => {
                        self.transaction = None;
                        Task::none()
                    }
                    Action::InstallSucceeded => {
                        self.refresh_installed_state();
                        Task::done(
                            crate::Message::Updates(UpdatesMessage::RefreshUpdates).into(),
                        )
                    }
                }
            }
        }
    }

    fn handle_updates(&mut self, message: UpdatesMessage) -> Task<Message> {
        match message {
            UpdatesMessage::RefreshUpdates => Task::none(),
            UpdatesMessage::Fetched(result) => match result {
                Ok(fetch) => {
                    let count = (fetch.repo.len() + fetch.aur.len()) as u32;
                    eprintln!(
                        "[pakajo] {} updates available (repo={} aur={})",
                        count,
                        fetch.repo.len(),
                        fetch.aur.len()
                    );
                    self.pending_updates = pakajo::updates::PendingUpdates {
                        repo: fetch.repo,
                        aur: fetch.aur,
                    };
                    self.updates_aur_error = fetch.aur_error;
                    self.pending_count = count;
                    self.updates_state = UpdatesState::Idle;
                    Task::none()
                }
                Err(msg) => {
                    eprintln!("[pakajo] updates checker failed: {msg}");
                    self.updates_state = UpdatesState::Error(msg);
                    Task::none()
                }
            },
        }
    }

    fn start_updates_check(&mut self) -> Task<Message> {
        if matches!(self.updates_state, UpdatesState::Loading) {
            return Task::none();
        }
        self.updates_state = UpdatesState::Loading;
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let result = pakajo::updates::pending_updates();
            let _ = tx.send(result);
        });
        Task::perform(
            async move {
                match rx.await {
                    Ok(Ok(fetch)) => Ok(fetch),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("updates check cancelled".to_string()),
                }
            },
            |result| Message::Updates(UpdatesMessage::Fetched(result)).into(),
        )
    }

    fn refresh_installed_state(&mut self) {
        if let Ok(config) = pacmanconf::Config::new()
            && let Ok(handle) = init_alpm(&config)
        {
            self.alpm = Some(handle);
        }
        if let Some(alpm) = &self.alpm {
            self.installed_names = Arc::new(pakajo::package::installed_names(alpm));
        }
        pakajo::search::apply_installed_to_results(&mut self.results, &self.installed_names);
        self.refresh_detail_installed();
    }

    fn refresh_detail_installed(&mut self) {
        if let DetailData::Ready { pkg, .. } = &self.detail {
            let installed = self
                .alpm
                .as_ref()
                .map(|a| pakajo::package::is_installed(a, &pkg.name))
                .unwrap_or(false);
            let pkg = pkg.clone();
            self.detail = DetailData::Ready { pkg, installed };
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
    Detail(DetailMessage),
    Transaction(TransactionMessage),
    Updates(UpdatesMessage),
}

#[derive(Clone, Debug)]
pub enum UpdatesMessage {
    RefreshUpdates,
    Fetched(Result<pakajo::updates::UpdatesFetch, String>),
}

#[derive(Clone, Debug)]
pub enum UpdatesState {
    Idle,
    Loading,
    Error(String),
}
