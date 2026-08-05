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
use transaction::{TransactionMessage, TransactionModel, transaction_view};

fn main() -> cosmic::iced::Result {
    let cli = cli::parse();
    cli::dispatch(cli);
    let settings = Settings::default();
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
    pub(crate) transacting: bool,
    pub(crate) transaction: Option<TransactionModel>,
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

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
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

        (
            PakajoApp {
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
                transacting: false,
                transaction: None,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Search(m) => self.handle_search(m),
            Message::Detail(m) => self.handle_detail(m),
            Message::Transaction(m) => self.handle_transaction(m),
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
        if let Some(model) = self.transaction.as_ref() {
            return container(transaction_view(model)).into();
        }
        let content = Column::new()
            .spacing(12)
            .push(search_bar(&self.query))
            .push(search_status_text(self.search_state, self.results.len()))
            .push(
                Row::new()
                    .push(scrollable(results_list(&self.results, self.selected_index)).width(384.))
                    .push(detail_view(&self.detail)),
            );

        container(content).into()
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
    Detail(DetailMessage),
    Transaction(TransactionMessage),
}
