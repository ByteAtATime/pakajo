mod background;
mod components;

use std::collections::HashSet;
use std::sync::Arc;

use cosmic::iced::widget::{Id, scrollable};
use cosmic::widget::{Column, container, popover, rectangle_tracker, text_input};
use cosmic::{
    Application,
    app::{self, Core, Settings, Task},
    executor,
    iced::{self, Event, Length, Subscription, event, keyboard},
};
use pakajo::aur::AurClient;
use pakajo::cli;
use pakajo::dashboard::DashboardMessage;
use pakajo::db::PackageDb;
use pakajo::pacman::handle;
use pakajo::search::engine::SearchEngine;

use background::begin_aur_sync_in_background;
use components::dashboard::DashboardState;
use components::detail::{DetailMessage, DetailPane, detail_view};
use components::footer;
use components::search::{ListRect, SearchMessage, SearchPane, search_input_id};
use components::sysupgrade::SysupgradeMessage;
use components::transaction::{Action, TransactionMessage, TxPane};
use components::updates::{RefreshKind, UpdatesMessage, UpdatesPane};

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

pub struct PakajoCtx {
    pub(crate) search_engine: Option<Arc<SearchEngine>>,
    pub(crate) db: Option<Arc<PackageDb>>,
    pub(crate) alpm: Option<alpm::Alpm>,
    pub(crate) aur_client: Option<Arc<AurClient>>,
    pub(crate) installed_names: Arc<HashSet<String>>,
    pub(crate) foreign_names: Arc<HashSet<String>>,
    pub(crate) group_index: Arc<Vec<(String, String)>>,
}

pub struct PakajoApp {
    core: Core,
    pub(crate) ctx: PakajoCtx,
    pub(crate) dashboard: DashboardState,
    pub(crate) search: SearchPane,
    pub(crate) detail: DetailPane,
    pub(crate) tx: TxPane,
    pub(crate) updates: UpdatesPane,
    pub(crate) page: Page,
    search_focus_pending: bool,
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
        let (alpm, installed_names) = match handle() {
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

        let ctx = PakajoCtx {
            search_engine,
            db,
            alpm,
            aur_client,
            installed_names,
            foreign_names: Arc::new(HashSet::new()),
            group_index,
        };
        if let Some(index) = &ctx.db {
            begin_aur_sync_in_background(index.clone(), ctx.search_engine.clone());
        }

        let mut app = PakajoApp {
            core,
            ctx,
            dashboard: DashboardState::default(),
            search: SearchPane::default(),
            detail: DetailPane::default(),
            tx: TxPane {
                transaction: None,
                show: false,
            },
            updates: UpdatesPane::default(),
            page: Page::Search,
            search_focus_pending: true,
        };
        let task = app.updates.restore_cache();
        let groups_task = Task::perform(
            async {
                handle()
                    .map(|handle| Arc::new(pakajo::package::group_index(&handle)))
                    .unwrap_or_else(|e| {
                        eprintln!("[pakajo] failed to index package groups: {e:#}");
                        Arc::new(Vec::new())
                    })
            },
            |index| Message::Search(SearchMessage::GroupsLoaded(index)).into(),
        );
        let dashboard_task = app.dashboard.refresh();
        (app, Task::batch([task, groups_task, dashboard_task]))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let focus = self.ensure_search_focus();
        let task = match message {
            Message::Search(m) => {
                let active = self.page == Page::Search && self.tx.overlay().is_none();
                self.search.update(m, &mut self.ctx, active)
            }
            Message::Dashboard(m) => self.dashboard.update(m, &mut self.ctx),
            Message::Detail(m) => {
                let tx_active = self.tx.is_active();
                self.detail.update(m, &self.ctx, tx_active)
            }
            Message::Transaction(m) => self.handle_transaction(m),
            Message::Updates(m) => self.updates.update(m),
            Message::Sysupgrade(_) => self.tx.start_sysupgrade(),
            Message::Navigate(page) => self.goto_page(page),
            Message::SearchFocus(focused) => {
                self.search_focus_pending &= !focused;
                Task::none()
            }
            Message::OpenTransaction => {
                self.tx.open();
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
                let dashboard = self.dashboard.refresh();
                let updates = if !self.tx.is_active() {
                    eprintln!("[pakajo] db.lck released, forcing updates recheck");
                    self.updates.start_check(RefreshKind::ExternalChange)
                } else {
                    Task::none()
                };
                Task::batch([dashboard, updates])
            }
        };
        Task::batch([focus, task])
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let ticking = self.tx.building();
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
        let content: Element<'_> = if let Some(t) = self.tx.overlay() {
            container(t.view())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            let page = match self.page {
                Page::Search => self.search_page(),
                Page::Updates => self.updates.page(self.tx.sysupgrade_checking()),
            };
            Column::new()
                .push(container(page).width(Length::Fill).height(Length::Fill))
                .push(divider::horizontal::default())
                .push(footer::footer(
                    self.tx.transaction.as_ref(),
                    self.updates.badge(),
                ))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        let mut pop = popover(content).modal(true);
        if let Some(dialog) = self.tx.dialog() {
            pop = pop.popup(dialog);
        }
        pop.into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::style::iced::application::style(
            &cosmic::theme::active(),
        ))
    }
}

impl PakajoApp {
    fn search_page(&self) -> Element<'_> {
        let active_target = self.tx.active_name();
        let detail = detail_view(
            &self.detail.data,
            active_target,
            self.detail.pending.is_some(),
            &self.detail.selected_optdeps,
            self.tx.is_active(),
            self.detail.optdep_hover.as_deref(),
        );
        self.search.view(self.dashboard.snapshot.as_ref(), detail)
    }

    fn handle_transaction(&mut self, message: TransactionMessage) -> Task<Message> {
        match message {
            TransactionMessage::Begin(request) => self.tx.begin(request, &self.ctx),
            other => {
                let action = self.tx.forward(other);
                match action {
                    Action::None => Task::none(),
                    Action::Run(task) => task,
                    Action::ViewClosed => {
                        self.tx.close();
                        Task::none()
                    }
                    Action::Finished => {
                        let was_sysupgrade = self.tx.sysupgrade_running();
                        self.tx.finish();
                        if was_sysupgrade {
                            return self.goto_page(Page::Updates);
                        }
                        Task::none()
                    }
                    Action::InstallSucceeded => {
                        self.refresh_installed_state();
                        let refresh = Task::done(
                            crate::Message::Updates(UpdatesMessage::RefreshUpdates).into(),
                        );
                        if self.tx.sysupgrade_running() {
                            self.tx.finish();
                            return Task::batch([refresh, self.goto_page(Page::Updates)]);
                        }
                        let dashboard = self.dashboard.refresh();
                        Task::batch([dashboard, refresh])
                    }
                }
            }
        }
    }

    fn refresh_installed_state(&mut self) {
        if let Ok(handle) = handle() {
            self.ctx.alpm = Some(handle);
        }
        if let Some(alpm) = &self.ctx.alpm {
            self.ctx.installed_names = Arc::new(pakajo::package::installed_names(alpm));
        }
        pakajo::search::apply_installed_to_results(
            &mut self.search.results,
            &self.ctx.installed_names,
        );
        self.detail.refresh_installed(&self.ctx);
    }

    pub(crate) fn goto_page(&mut self, page: Page) -> Task<Message> {
        self.page = page;
        self.search_focus_pending = matches!(page, Page::Search);
        self.search.scroller.reset_offset();
        scroll_to_top()
    }

    fn ensure_search_focus(&self) -> Task<Message> {
        if !self.search_focus_pending
            || !matches!(self.page, Page::Search)
            || self.tx.overlay().is_some()
        {
            return Task::none();
        }
        cosmic::iced::runtime::widget::operation::is_focused(search_input_id())
            .then(|focused| match focused {
                true => cosmic::iced::Task::done(true),
                false => text_input::focus(search_input_id()).map(|_: ()| false),
            })
            .map(|focused| cosmic::Action::App(Message::SearchFocus(focused)))
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
    SearchFocus(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Search,
    Updates,
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
