use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use cosmic::widget::{Column, Row, button, container, scrollable, text, text_input};
use cosmic::{
    Application, Element,
    app::{self, Core, Settings, Task},
    executor,
};
use pakajo::cli;
use pakajo::local_index::{LocalIndex, RefreshOutcome};
use pakajo::pacman::init_alpm;
use pakajo::search::SearchResult;
use pakajo::search::engine::SearchEngine;

const AUR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

fn main() -> cosmic::iced::Result {
    let cli = cli::parse();
    cli::dispatch(cli);
    let settings = Settings::default();
    let flags = ();
    app::run::<PakajoApp>(settings, flags)
}

pub struct PakajoApp {
    core: Core,
    search_engine: Option<Arc<SearchEngine>>,
    local_index: Option<Arc<LocalIndex>>,
    installed_names: Arc<HashSet<String>>,
    group_index: Arc<Vec<(String, String)>>,
    query: String,
    results: Vec<SearchResult>,
    search_state: SearchState,
    search_seq: u64,
    selected_index: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum SearchState {
    Idle,
    Searching,
    Done,
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

        let (installed_names, group_index) = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(handle) => (
                Arc::new(pakajo::package::installed_names(&handle)),
                Arc::new(pakajo::pacman::collect_group_index(&handle)),
            ),
            Err(e) => {
                eprintln!("[pakajo] failed to snapshot installed packages and groups: {e:#}");
                (Arc::new(HashSet::new()), Arc::new(Vec::new()))
            }
        };

        if let Some(index) = &local_index {
            begin_aur_sync_in_background(index.clone(), search_engine.clone());
        }

        (
            PakajoApp {
                core,
                search_engine,
                local_index,
                installed_names,
                group_index,
                query: String::new(),
                results: Vec::new(),
                search_state: SearchState::Idle,
                search_seq: 0,
                selected_index: None,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Search(SearchMessage::QueryChanged(text)) => {
                self.query = text.clone();
                self.search_seq = self.search_seq.wrapping_add(1);
                let seq = self.search_seq;

                if text.trim().is_empty() {
                    self.results.clear();
                    self.selected_index = None;
                    self.search_state = SearchState::Idle;
                    return Task::none();
                }

                self.search_state = SearchState::Searching;
                let engine = self.search_engine.clone();
                let local_index = self.local_index.clone();
                let installed = self.installed_names.clone();
                let group_index = self.group_index.clone();

                Task::perform(
                    async move { execute_search_for(engine, local_index, group_index, installed, text) },
                    move |results| {
                        Message::Search(SearchMessage::ResultsReady { seq, results }).into()
                    },
                )
            }
            Message::Search(SearchMessage::ResultsReady { seq, results }) => {
                if seq == self.search_seq {
                    self.results = results;
                    self.selected_index =
                        if self.results.is_empty() { None } else { Some(0) };
                    self.search_state = SearchState::Done;
                }
                Task::none()
            }
            Message::Search(SearchMessage::SelectDelta(delta)) => {
                if let Some(i) =
                    next_selected_index(self.results.len(), self.selected_index, delta)
                {
                    self.selected_index = Some(i);
                }
                Task::none()
            }
            Message::Search(SearchMessage::SelectIndex(i)) => {
                if i < self.results.len() {
                    self.selected_index = Some(i);
                }
                Task::none()
            }
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
                cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::ArrowDown) => {
                    Some(Message::Search(SearchMessage::SelectDelta(1)))
                }
                _ => None,
            },
            _ => None,
        })
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let search_input = text_input("Search packages", &self.query)
            .on_input(|s| Message::Search(SearchMessage::QueryChanged(s)));

        let header_text = if self.search_state == SearchState::Searching {
            "Searching...".to_string()
        } else {
            format!("{} result(s)", self.results.len())
        };

        let mut list = Column::new().spacing(6);
        for (index, result) in self.results.iter().enumerate() {
            let is_selected = self.selected_index == Some(index);
            list = list.push(result_row(result, index, is_selected));
        }

        let content = Column::new()
            .spacing(12)
            .push(search_input)
            .push(text(header_text))
            .push(scrollable(list));

        container(content).into()
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
}

#[derive(Clone, Debug)]
pub enum SearchMessage {
    QueryChanged(String),
    ResultsReady {
        seq: u64,
        results: Vec<SearchResult>,
    },
    SelectDelta(i32),
    SelectIndex(usize),
}

fn result_row(result: &SearchResult, index: usize, is_selected: bool) -> Element<'_, Message> {
    let badge = result.repo.as_deref().unwrap_or("aur");

    let top = Row::new()
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .push(text(result.name.clone()).font(cosmic::font::semibold()))
        .push(text(badge.to_string()))
        .push(text(result.version.clone()))
        .push_maybe(result.installed.then(|| text("installed")));

    let content = Column::new()
        .spacing(4)
        .push(top)
        .push_maybe(result.description.as_ref().map(|d| text(d.clone())));

    button::custom(content)
        .on_press(Message::Search(SearchMessage::SelectIndex(index)))
        .selected(is_selected)
        .class(cosmic::theme::Button::ListItem([0.0; 4]))
        .width(cosmic::iced::Length::Fill)
        .into()
}

fn execute_search_for(
    search_engine: Option<Arc<SearchEngine>>,
    local_index: Option<Arc<LocalIndex>>,
    group_index: Arc<Vec<(String, String)>>,
    installed: Arc<HashSet<String>>,
    text: String,
) -> Vec<SearchResult> {
    let (Some(engine), Some(local)) = (search_engine.as_ref(), local_index.as_ref()) else {
        return Vec::new();
    };
    pakajo::search::dispatch_search(engine, local, &installed, &text, &group_index)
}

fn begin_aur_sync_in_background(
    local_index: Arc<LocalIndex>,
    search_engine: Option<Arc<SearchEngine>>,
) {
    std::thread::spawn(move || {
        let reniced = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) };
        if reniced != 0 {
            eprintln!(
                "[pakajo] failed to renice aur sync worker: {}",
                std::io::Error::last_os_error()
            );
        }

        if let Some(age) = local_index.last_refreshed_age()
            && age < AUR_SYNC_MIN_INTERVAL
        {
            eprintln!(
                "[pakajo] skipping aur sync (last refresh {}h ago)",
                age.as_secs() / 3600
            );
            return;
        }

        let handle = match pacmanconf::Config::new()
            .context("failed to read pacman config")
            .and_then(|cfg| init_alpm(&cfg))
        {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed to init alpm: {e:#}");
                return;
            }
        };

        match local_index.refresh(&handle) {
            Ok(RefreshOutcome::NotModified) => {
                eprintln!("[pakajo] aur index up to date");
            }
            Ok(RefreshOutcome::Updated {
                aur_count,
                repo_count,
                skipped,
            }) => {
                eprintln!(
                    "[pakajo] indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})"
                );
                if let Some(engine) = search_engine.as_ref() {
                    let _ = engine.ensure_fresh();
                }
            }
            Err(e) => {
                eprintln!("[pakajo] aur background sync failed: {e:#}");
            }
        }
    });
}

fn next_selected_index(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    let max = len.checked_sub(1)?;
    let cur = current.unwrap_or(0);
    let next = (cur as i32 + delta).clamp(0, max as i32) as usize;
    if current == Some(next) {
        None
    } else {
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::next_selected_index;

    #[test]
    fn next_selected_index_empty_returns_none() {
        assert_eq!(next_selected_index(0, None, 1), None);
    }

    #[test]
    fn next_selected_index_clamps_to_bounds() {
        assert_eq!(next_selected_index(5, None, -3), Some(0));
        assert_eq!(next_selected_index(5, None, 100), Some(4));
    }

    #[test]
    fn next_selected_index_noop_returns_none() {
        assert_eq!(next_selected_index(5, Some(4), 1), None);
        assert_eq!(next_selected_index(5, Some(0), -1), None);
    }

    #[test]
    fn next_selected_index_moves() {
        assert_eq!(next_selected_index(5, Some(2), -1), Some(1));
        assert_eq!(next_selected_index(5, Some(2), 1), Some(3));
    }
}
