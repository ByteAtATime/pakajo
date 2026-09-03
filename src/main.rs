mod background;
mod components;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use cosmic::widget::{Column, Row, container};
use cosmic::{
    Application, Element,
    app::{self, Core, Settings, Task},
    executor,
};
use pakajo::aur::AurClient;
use pakajo::cli;
use pakajo::db::PackageDb;
use pakajo::pacman::init_alpm;
use pakajo::search::SearchFilter;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

use background::begin_aur_sync_in_background;
use components::detail::{DetailData, DetailMessage, detail_view};
use components::search::{
    ListRect, SearchMessage, SearchState, SelectionScroller, results_scroller, search_bar,
    search_input_id, search_status_bar,
};
use components::sysupgrade::SysupgradeMessage;
use components::transaction::review::ReviewModel;
use components::transaction::{Action, Transaction, TransactionMessage};
use components::updates::{UpdatesMessage, UpdatesState};

use crate::components::divider::{divider, vdivider};

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
    pub(crate) db: Option<Arc<PackageDb>>,
    pub(crate) alpm: Option<alpm::Alpm>,
    pub(crate) aur_client: Option<Arc<AurClient>>,
    pub(crate) installed_names: Arc<HashSet<String>>,
    pub(crate) group_index: Arc<Vec<(String, String)>>,
    pub(crate) query: String,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) search_state: SearchState,
    pub(crate) search_seq: u64,
    pub(crate) search_filter: SearchFilter,
    pub(crate) selected_index: Option<usize>,
    pub(crate) scroller: SelectionScroller,
    pub(crate) detail: DetailData,
    pub(crate) detail_seq: u64,
    pub(crate) detail_pending: Option<u64>,
    pub(crate) transaction: Option<Transaction>,
    pub(crate) updates_state: UpdatesState,
    pub(crate) pending_updates: pakajo::updates::PendingUpdates,
    pub(crate) pending_count: u32,
    pub(crate) updates_aur_error: Option<String>,
    pub(crate) page: Page,
    pub(crate) sysupgrade_preview: Option<pakajo::dry_run::SysupgradePreview>,
    pub(crate) sysupgrade_preview_error: Option<String>,
    pub(crate) sysupgrade_preview_in_flight: bool,
    initial_focus_done: bool,
    pub(crate) sysupgrade_aur_targets: Vec<String>,
    pub(crate) sysupgrade_review: Option<ReviewModel>,
    pub(crate) pkgbuild_review_index: usize,
    pub(crate) active_sysupgrade_phase: Option<pakajo::transaction_state::SysupgradePhase>,
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
        let search_engine = PackageDb::db_path()
            .ok()
            .and_then(|p| SearchEngine::new(p).ok())
            .map(Arc::new);
        let db = PackageDb::db_path()
            .ok()
            .and_then(|p| {
                PackageDb::open(&p)
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

        if let Some(index) = &db {
            begin_aur_sync_in_background(index.clone(), search_engine.clone());
        }

        let mut app = PakajoApp {
            core,
            search_engine,
            db,
            alpm,
            aur_client,
            installed_names,
            group_index,
            query: String::new(),
            results: Vec::new(),
            search_state: SearchState::Idle,
            search_seq: 0,
            search_filter: SearchFilter::All,
            selected_index: None,
            scroller: SelectionScroller::new(),
            detail: DetailData::None,
            detail_seq: 0,
            detail_pending: None,
            transaction: None,
            updates_state: UpdatesState::Idle,
            pending_updates: pakajo::updates::PendingUpdates {
                repo: Vec::new(),
                aur: Vec::new(),
            },
            pending_count: 0,
            updates_aur_error: None,
            page: Page::Search,
            sysupgrade_preview: None,
            sysupgrade_preview_error: None,
            sysupgrade_preview_in_flight: false,
            initial_focus_done: false,
            sysupgrade_aur_targets: Vec::new(),
            sysupgrade_review: None,
            pkgbuild_review_index: 0,
            active_sysupgrade_phase: None,
        };
        let task = app.start_updates_check();
        (app, task)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let focus = if self.initial_focus_done {
            Task::none()
        } else {
            self.initial_focus_done = true;
            cosmic::widget::text_input::focus(search_input_id())
        };
        let task = match message {
            Message::Search(m) => self.handle_search(m),
            Message::Detail(m) => self.handle_detail(m),
            Message::Transaction(m) => self.handle_transaction(m),
            Message::Updates(m) => self.handle_updates(m),
            Message::Sysupgrade(m) => self.handle_sysupgrade(m),
            Message::Navigate(page) => self.goto_page(page),
            Message::DbLockReleased => {
                eprintln!("[pakajo] db.lck released, refreshing installed state");
                self.refresh_installed_state();
                Task::none()
            }
        };
        Task::batch([focus, task])
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        cosmic::iced::Subscription::batch([
            cosmic::iced::event::listen_with(|event, _status, _id| match event {
                cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                    key,
                    ..
                }) => match key {
                    cosmic::iced::keyboard::Key::Named(
                        cosmic::iced::keyboard::key::Named::ArrowUp,
                    ) => Some(Message::Search(SearchMessage::SelectDelta(-1))),
                    cosmic::iced::keyboard::Key::Named(
                        cosmic::iced::keyboard::key::Named::ArrowDown,
                    ) => Some(Message::Search(SearchMessage::SelectDelta(1))),
                    _ => None,
                },
                _ => None,
            }),
            background::db_lock_watcher_subscription(),
            cosmic::widget::rectangle_tracker::subscription::<ListRect, ListRect>(
                ListRect::Viewport,
            )
            .map(|(_, update)| Message::Search(SearchMessage::Rects(update))),
        ])
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if let Some(t) = self.transaction.as_ref()
            && !t.is_checking()
        {
            return container(t.view())
                .width(cosmic::iced::Length::Fill)
                .height(cosmic::iced::Length::Fill)
                .into();
        }
        match self.page {
            Page::Search => self.search_page(),
            Page::Updates => self.updates_page(),
            Page::Resolve => self.resolve_page(),
            Page::PkgbuildReview => self.pkgbuild_review_page(),
            Page::Confirm => self.confirm_page(),
        }
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        self.transaction.as_ref().and_then(|t| t.dialog())
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::style::iced::application::style(
            &cosmic::theme::active(),
        ))
    }
}

impl PakajoApp {
    fn search_page(&self) -> Element<'_, Message> {
        let checking = self.transaction.as_ref().map(|t| t.name());
        let header = Row::new()
            .spacing(8)
            .push(search_bar(&self.query))
            .push(self.updates_badge());
        let content = Column::new()
            .spacing(12)
            .push(header)
            .push(search_status_bar(
                self.search_state,
                self.results.len(),
                self.search_filter,
                &self.query,
            ))
            .push(
                Column::new().push(divider()).push(
                    Row::new()
                        .push(results_scroller(
                            &self.results,
                            self.selected_index,
                            &self.scroller,
                        ))
                        .push(vdivider())
                        .push(detail_view(
                            &self.detail,
                            checking,
                            self.detail_pending.is_some(),
                        )),
                ),
            );

        container(content).into()
    }

    fn handle_transaction(&mut self, message: TransactionMessage) -> Task<Message> {
        match message {
            TransactionMessage::StartInstall => {
                if self
                    .transaction
                    .as_ref()
                    .is_some_and(Transaction::is_active)
                    || self.detail_pending.is_some()
                {
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
            TransactionMessage::StartRemove => {
                if self
                    .transaction
                    .as_ref()
                    .is_some_and(Transaction::is_active)
                    || self.detail_pending.is_some()
                {
                    return Task::none();
                }
                let (name, source) = match &self.detail {
                    DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source),
                    _ => return Task::none(),
                };
                let (txn, task) = Transaction::start_remove(name, source);
                self.transaction = Some(txn);
                task
            }
            other => {
                let action = match self.transaction.as_mut() {
                    Some(t) => t.update(
                        other,
                        self.active_sysupgrade_phase,
                        &self.sysupgrade_aur_targets,
                    ),
                    None => Action::None,
                };
                match action {
                    Action::None => Task::none(),
                    Action::Run(task) => task,
                    Action::ContinueAur(targets) => {
                        self.active_sysupgrade_phase =
                            Some(pakajo::transaction_state::SysupgradePhase::Aur);
                        eprintln!(
                            "[pakajo] sysupgrade continuing to aur phase: {} targets",
                            targets.len()
                        );
                        self.refresh_installed_state();
                        let refresh = Task::done(
                            crate::Message::Updates(UpdatesMessage::RefreshUpdates).into(),
                        );
                        let (transaction, task) = Transaction::start_sysupgrade_aur(targets);
                        self.transaction = Some(transaction);
                        Task::batch([refresh, task])
                    }
                    Action::Finished => {
                        self.transaction = None;
                        if self.active_sysupgrade_phase.is_some() {
                            self.sysupgrade_preview = None;
                            self.sysupgrade_review = None;
                            self.sysupgrade_preview_error = None;
                            self.sysupgrade_preview_in_flight = false;
                            self.sysupgrade_aur_targets.clear();
                            self.pkgbuild_review_index = 0;
                            self.active_sysupgrade_phase = None;
                            return Task::batch([self.goto_page(crate::Page::Updates)]);
                        }
                        Task::none()
                    }
                    Action::InstallSucceeded => {
                        self.refresh_installed_state();
                        let refresh = Task::done(
                            crate::Message::Updates(UpdatesMessage::RefreshUpdates).into(),
                        );
                        if self.active_sysupgrade_phase.is_some() {
                            self.sysupgrade_preview = None;
                            self.sysupgrade_review = None;
                            self.sysupgrade_preview_error = None;
                            self.sysupgrade_preview_in_flight = false;
                            self.sysupgrade_aur_targets.clear();
                            self.pkgbuild_review_index = 0;
                            self.active_sysupgrade_phase = None;
                            self.transaction = None;
                            return Task::batch([refresh, self.goto_page(crate::Page::Updates)]);
                        }
                        refresh
                    }
                }
            }
        }
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

    pub(crate) fn goto_page(&mut self, page: Page) -> Task<Message> {
        self.page = page;
        self.scroller.reset_offset();
        scroll_to_top()
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
    Detail(DetailMessage),
    Transaction(TransactionMessage),
    Updates(UpdatesMessage),
    Sysupgrade(SysupgradeMessage),
    Navigate(Page),
    DbLockReleased,
}

#[derive(Clone, Copy, Debug)]
pub enum Page {
    Search,
    Updates,
    Resolve,
    PkgbuildReview,
    Confirm,
}

pub(crate) fn page_scroll_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("page-scroll")
}

pub(crate) fn scroll_to_top() -> Task<Message> {
    cosmic::iced::widget::scrollable::scroll_to(
        page_scroll_id(),
        cosmic::iced::widget::scrollable::AbsoluteOffset {
            x: Some(0.0),
            y: Some(0.0),
        },
    )
}
