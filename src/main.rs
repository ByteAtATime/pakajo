mod background;
mod components;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use cosmic::iced::widget::{Id, scrollable};
use cosmic::widget::{Column, Row, container, popover, rectangle_tracker, text_input};
use cosmic::{
    Application,
    app::{self, Core, Settings, Task},
    executor,
    iced::{self, Event, Length, Subscription, event, keyboard},
};
use pakajo::aur::AurClient;
use pakajo::cli;
use pakajo::dashboard::{DashboardMessage, DashboardSnapshot};
use pakajo::db::PackageDb;
use pakajo::pacman::init_alpm;
use pakajo::search::SearchFilter;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

use background::begin_aur_sync_in_background;
use components::dashboard::dashboard_view;
use components::detail::{DetailData, DetailMessage, detail_view};
use components::footer;
use components::search::{
    ListRect, SearchMessage, SearchState, SelectionScroller, results_scroller, search_bar,
    search_input_id, search_status_bar,
};
use components::sysupgrade::SysupgradeMessage;
use components::transaction::review::ReviewModel;
use components::transaction::{Action, Transaction, TransactionMessage};
use components::updates::{RefreshKind, UpdatesMessage, UpdatesState};

use cosmic::widget::divider;

pub type Element<'a> = cosmic::Element<'a, Message>;

fn main() -> iced::Result {
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some(pakajo::dispatch::operation::MARKER) {
        std::process::exit(pakajo::dispatch::child::run(&argv[2..]));
    }
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
    pub(crate) foreign_names: Arc<HashSet<String>>,
    pub(crate) dashboard: Option<DashboardSnapshot>,
    pub(crate) dashboard_seq: u64,
    pub(crate) news: pakajo::news::NewsState,
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
    pub(crate) show_transaction: bool,
    pub(crate) updates_state: UpdatesState,
    pub(crate) pending_updates: pakajo::updates::PendingUpdates,
    pub(crate) pending_count: u32,
    pub(crate) updates_aur_error: Option<String>,
    pub(crate) last_cache: Option<pakajo::updates::UpdatesCache>,
    pub(crate) updates_refreshing: bool,
    pub(crate) updates_refresh_error: Option<String>,
    pub(crate) pending_force_refresh: Option<RefreshKind>,
    pub(crate) page: Page,
    pub(crate) sysupgrade_preview: Option<pakajo::dry_run::SysupgradePreview>,
    pub(crate) sysupgrade_preview_error: Option<String>,
    pub(crate) sysupgrade_preview_in_flight: bool,
    initial_focus_done: bool,
    pub(crate) sysupgrade_aur_targets: Vec<String>,
    pub(crate) sysupgrade_review: Option<ReviewModel>,
    pub(crate) pkgbuild_review_index: usize,
    pub(crate) active_sysupgrade_phase: Option<pakajo::progress::SysupgradePhase>,
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
        core.window.sharp_corners = true;
        core.window.use_template = false;
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

        let group_index = Arc::new(Vec::new());
        let (alpm, installed_names) = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(handle) => {
                let installed = Arc::new(pakajo::package::installed_names(&handle));
                (Some(handle), installed)
            }
            Err(e) => {
                eprintln!("[pakajo] failed to snapshot installed packages: {e:#}");
                (None, Arc::new(HashSet::new()))
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
            foreign_names: Arc::new(HashSet::new()),
            dashboard: None,
            dashboard_seq: 0,
            news: pakajo::news::NewsState::Loading,
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
            show_transaction: false,
            updates_state: UpdatesState::Idle,
            pending_updates: pakajo::updates::PendingUpdates {
                repo: Vec::new(),
                aur: Vec::new(),
            },
            pending_count: 0,
            updates_aur_error: None,
            last_cache: None,
            updates_refreshing: false,
            updates_refresh_error: None,
            pending_force_refresh: None,
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
        let task = match pakajo::updates::load_cached() {
            Some(cache) => {
                app.last_cache = Some(cache);
                let cache = app.last_cache.as_ref().expect("cache stored above");
                let now = pakajo::updates::now_unix_seconds();
                app.pending_updates = pakajo::updates::PendingUpdates {
                    repo: cache.repo.clone(),
                    aur: cache.aur.clone(),
                };
                app.pending_count = (cache.repo.len() + cache.aur.len()) as u32;
                app.updates_state = UpdatesState::Idle;
                app.updates_aur_error = None;
                let skip = !cache.repo_stale(now)
                    && !cache.devel_stale(now)
                    && pakajo::updates::localdb_unchanged_since(cache.checked_at);
                if skip {
                    eprintln!(
                        "[pakajo] updates cache fresh (repo age {}s, devel age {}s), skipping revalidation",
                        now.saturating_sub(cache.checked_at),
                        now.saturating_sub(cache.devel_checked_at)
                    );
                    Task::none()
                } else {
                    eprintln!(
                        "[pakajo] serving cached updates (repo={} aur={})",
                        cache.repo.len(),
                        cache.aur.len()
                    );
                    app.start_updates_check(RefreshKind::Launch)
                }
            }
            None => app.start_updates_check(RefreshKind::Launch),
        };
        let groups_task = Task::perform(
            async {
                pacmanconf::Config::new()
                    .map_err(anyhow::Error::new)
                    .and_then(|cfg| init_alpm(&cfg))
                    .map(|handle| Arc::new(pakajo::package::group_index(&handle)))
                    .unwrap_or_else(|e| {
                        eprintln!("[pakajo] failed to index package groups: {e:#}");
                        Arc::new(Vec::new())
                    })
            },
            |index| Message::Search(SearchMessage::GroupsLoaded(index)).into(),
        );
        let dashboard_task = app.start_dashboard_refresh();
        let news_task = app.start_news_fetch();
        (
            app,
            Task::batch([task, groups_task, dashboard_task, news_task]),
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let focus = if self.initial_focus_done {
            Task::none()
        } else {
            self.initial_focus_done = true;
            text_input::focus(search_input_id())
        };
        let task = match message {
            Message::Search(m) => self.handle_search(m),
            Message::Dashboard(m) => self.handle_dashboard(m),
            Message::Detail(m) => self.handle_detail(m),
            Message::Transaction(m) => self.handle_transaction(m),
            Message::Updates(m) => self.handle_updates(m),
            Message::Sysupgrade(m) => self.handle_sysupgrade(m),
            Message::Navigate(page) => self.goto_page(page),
            Message::OpenTransaction => {
                self.show_transaction = true;
                Task::none()
            }
            Message::OpenUrl(url) => {
                std::thread::spawn(move || {
                    if let Err(e) = std::process::Command::new("xdg-open").arg(&url).spawn() {
                        eprintln!("[pakajo] failed to open {url}: {e}");
                    }
                });
                Task::none()
            }
            Message::DbLockReleased => {
                eprintln!("[pakajo] db.lck released, refreshing installed state");
                self.refresh_installed_state();
                let dashboard = self.start_dashboard_refresh();
                let updates = if self.transaction.as_ref().is_none_or(|t| !t.is_active()) {
                    eprintln!("[pakajo] db.lck released, forcing updates recheck");
                    self.start_updates_check(RefreshKind::ExternalChange)
                } else {
                    Task::none()
                };
                Task::batch([dashboard, updates])
            }
        };
        Task::batch([focus, task])
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let ticking = self
            .transaction
            .as_ref()
            .is_some_and(|t| t.is_active() && t.building());
        let tick = if ticking {
            cosmic::iced::time::every(std::time::Duration::from_secs(1))
                .map(|t| Message::Transaction(TransactionMessage::Tick(t)))
        } else {
            Subscription::none()
        };
        Subscription::batch([
            event::listen_with(|event, _status, _id| match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => match key {
                    keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                        Some(Message::Search(SearchMessage::SelectDelta(-1)))
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                        Some(Message::Search(SearchMessage::SelectDelta(1)))
                    }
                    _ => None,
                },
                _ => None,
            }),
            background::db_lock_watcher_subscription(),
            rectangle_tracker::subscription::<ListRect, ListRect>(ListRect::Viewport)
                .map(|(_, update)| Message::Search(SearchMessage::Rects(update))),
            tick,
        ])
    }

    fn view(&self) -> Element<'_> {
        let content: Element<'_> = if let Some(t) = self.overlay_transaction() {
            container(t.view())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            let page = match self.page {
                Page::Search => self.search_page(),
                Page::Updates => self.updates_page(),
                Page::Resolve => self.resolve_page(),
                Page::PkgbuildReview => self.pkgbuild_review_page(),
                Page::Confirm => self.confirm_page(),
            };
            Column::new()
                .push(container(page).width(Length::Fill).height(Length::Fill))
                .push(divider::horizontal::default())
                .push(footer::footer(
                    self.transaction.as_ref(),
                    self.updates_badge(),
                ))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        let mut pop = popover(content).modal(true);
        if let Some(dialog) = self.dialog() {
            pop = pop.popup(dialog);
        }
        pop.into()
    }

    fn dialog(&self) -> Option<Element<'_>> {
        self.transaction.as_ref().and_then(|t| t.dialog())
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::style::iced::application::style(
            &cosmic::theme::active(),
        ))
    }
}

impl PakajoApp {
    fn search_page(&self) -> Element<'_> {
        let active_target = self
            .transaction
            .as_ref()
            .filter(|t| t.is_active())
            .map(|t| t.name());
        let spacing = cosmic::theme::spacing();
        let page_padding = spacing.space_s as f32;
        let header = container(search_bar(&self.query)).padding([
            spacing.space_xs as f32,
            page_padding,
            0.0,
            page_padding,
        ]);
        if self.query.trim().is_empty() {
            let content = Column::new()
                .spacing(spacing.space_xs as f32)
                .push(header)
                .push(dashboard_view(self.dashboard.as_ref(), &self.news));
            return container(content).into();
        }
        let content = Column::new()
            .spacing(spacing.space_xs as f32)
            .push(header)
            .push(search_status_bar(
                self.search_state,
                self.results.len(),
                self.search_filter,
                &self.query,
            ))
            .push(
                Column::new().push(divider::horizontal::default()).push(
                    Row::new()
                        .push(results_scroller(
                            &self.results,
                            self.selected_index,
                            &self.scroller,
                        ))
                        .push(divider::vertical::default())
                        .push(detail_view(
                            &self.detail,
                            active_target,
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
                    DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source()),
                    _ => return Task::none(),
                };
                let (txn, task) = Transaction::start(name, source);
                self.transaction = Some(txn);
                self.show_transaction = false;
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
                    DetailData::Ready { pkg, .. } => (pkg.name.clone(), pkg.source()),
                    _ => return Task::none(),
                };
                let (txn, task) = Transaction::start_remove(name, source);
                self.transaction = Some(txn);
                self.show_transaction = false;
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
                        self.active_sysupgrade_phase = Some(pakajo::progress::SysupgradePhase::Aur);
                        eprintln!(
                            "[pakajo] sysupgrade continuing to aur phase: {} targets",
                            targets.len()
                        );
                        self.refresh_installed_state();
                        let dashboard = self.start_dashboard_refresh();
                        let refresh = Task::done(
                            crate::Message::Updates(UpdatesMessage::RefreshUpdates).into(),
                        );
                        let (transaction, task) = Transaction::start_sysupgrade_aur(targets);
                        self.transaction = Some(transaction);
                        Task::batch([dashboard, refresh, task])
                    }
                    Action::ViewClosed => {
                        self.show_transaction = false;
                        Task::none()
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
                        let dashboard = self.start_dashboard_refresh();
                        Task::batch([dashboard, refresh])
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

    fn start_dashboard_refresh(&mut self) -> Task<Message> {
        self.dashboard_seq = self.dashboard_seq.wrapping_add(1);
        let seq = self.dashboard_seq;
        crate::components::task::blocking_task(
            pakajo::dashboard::gather_dashboard,
            "dashboard refresh cancelled",
            move |result| match result {
                Ok((foreign, snapshot)) => {
                    crate::Message::Dashboard(DashboardMessage::SnapshotReady {
                        seq,
                        foreign,
                        snapshot,
                    })
                    .into()
                }
                Err(error) => {
                    crate::Message::Dashboard(DashboardMessage::LoadFailed { seq, error }).into()
                }
            },
        )
    }

    fn start_news_fetch(&self) -> Task<Message> {
        crate::components::task::blocking_task(
            || {
                pakajo::news::fetch_news().inspect_err(|e| {
                    eprintln!("[pakajo] news fetch failed: {e:#}");
                })
            },
            "news fetch cancelled",
            |result| crate::Message::Dashboard(DashboardMessage::NewsLoaded(result)).into(),
        )
    }

    fn handle_dashboard(&mut self, message: DashboardMessage) -> Task<Message> {
        match message {
            DashboardMessage::SnapshotReady {
                seq,
                foreign,
                snapshot,
            } => {
                if seq != self.dashboard_seq {
                    return Task::none();
                }
                self.foreign_names = Arc::new(foreign);
                self.dashboard = Some(snapshot);
                Task::none()
            }
            DashboardMessage::LoadFailed { .. } => Task::none(),
            DashboardMessage::NewsLoaded(result) => {
                self.news = match result {
                    Ok(items) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        pakajo::news::NewsState::Ready(pakajo::news::build_news_rows(items, now))
                    }
                    Err(_) => pakajo::news::NewsState::Unavailable,
                };
                Task::none()
            }
        }
    }

    pub(crate) fn goto_page(&mut self, page: Page) -> Task<Message> {
        self.page = page;
        self.scroller.reset_offset();
        scroll_to_top()
    }

    fn overlay_transaction(&self) -> Option<&Transaction> {
        self.transaction.as_ref().filter(|t| {
            (t.is_sysupgrade() && !t.is_checking()) || (self.show_transaction && !t.is_sysupgrade())
        })
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
    Dashboard(DashboardMessage),
    Detail(DetailMessage),
    Transaction(TransactionMessage),
    Updates(UpdatesMessage),
    Sysupgrade(SysupgradeMessage),
    Navigate(Page),
    OpenTransaction,
    OpenUrl(String),
    DbLockReleased,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Search,
    Updates,
    Resolve,
    PkgbuildReview,
    Confirm,
}

pub(crate) fn page_scroll_id() -> Id {
    Id::new("page-scroll")
}

pub(crate) fn scroll_to_top() -> Task<Message> {
    scrollable::scroll_to(
        page_scroll_id(),
        scrollable::AbsoluteOffset {
            x: Some(0.0),
            y: Some(0.0),
        },
    )
}
