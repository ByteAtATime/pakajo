use std::time::Duration;

use cosmic::app::Task;
use cosmic::widget::{Column, Row, button, text};
use futures::SinkExt as _;

use pakajo::package::{Package, PackageSource};
use pakajo::pacman::{find_groups, find_pkg};

use crate::transaction::TransactionMessage;

pub const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub enum DetailData {
    None,
    Loading,
    Ready {
        pkg: Package,
        installed: bool,
    },
    Error(String),
    Group {
        name: String,
        members: Vec<GroupMember>,
    },
}

#[derive(Clone, Debug)]
pub struct GroupMember {
    pub name: String,
    pub description: Option<String>,
    pub installed: bool,
}

#[derive(Clone)]
pub enum DetailMessage {
    DetailReady { seq: u64, pkg: Package },
    DetailFailed { seq: u64, message: String },
}

impl std::fmt::Debug for DetailMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DetailReady { seq, .. } => {
                f.debug_struct("DetailReady").field("seq", seq).finish()
            }
            Self::DetailFailed { seq, message } => f
                .debug_struct("DetailFailed")
                .field("seq", seq)
                .field("message", message)
                .finish(),
        }
    }
}

pub fn detail_view<'a>(
    detail: &'a DetailData,
    checking: Option<&'a str>,
) -> cosmic::Element<'a, crate::Message> {
    match detail {
        DetailData::None => text("Select a package").into(),
        DetailData::Loading => text("Loading...").into(),
        DetailData::Error(msg) => text(msg.clone()).into(),
        DetailData::Group { name, members } => {
            let mut col = Column::new()
                .spacing(6)
                .push(text(name.clone()).font(cosmic::font::semibold()));
            for member in members {
                let mut row = Row::new()
                    .spacing(8)
                    .push(text(member.name.clone()).font(cosmic::font::semibold()))
                    .push_maybe(member.description.as_ref().map(|d| text(d.clone())));
                if member.installed {
                    row = row.push(text("installed"));
                }
                col = col.push(row);
            }
            col.into()
        }
        DetailData::Ready { pkg, installed } => {
            let (label, intent) = if *installed {
                (
                    "Remove",
                    crate::Message::Transaction(TransactionMessage::StartRemove),
                )
            } else {
                (
                    "Install",
                    crate::Message::Transaction(TransactionMessage::StartInstall),
                )
            };
            let busy = checking == Some(pkg.name.as_str());
            let action = if busy {
                button::custom(text("Loading..."))
            } else {
                button::custom(text(label)).on_press(intent)
            };
            Column::new()
                .spacing(6)
                .push(text(pkg.name.clone()).font(cosmic::font::semibold()))
                .push(text(pkg.version.clone()))
                .push_maybe(pkg.description.as_ref().map(|d| text(d.clone())))
                .push(action)
                .into()
        }
    }
}

impl crate::PakajoApp {
    pub(crate) fn load_detail(
        &mut self,
        name: String,
        source: PackageSource,
    ) -> cosmic::app::Task<crate::Message> {
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let seq = self.detail_seq;
        self.detail = DetailData::Loading;

        match source {
            PackageSource::Repo => {
                let resolved = self
                    .alpm
                    .as_ref()
                    .and_then(|alpm| find_pkg(alpm, &name).map(Package::from));
                match resolved {
                    Some(pkg) => self.set_detail_pkg(pkg),
                    None => self.detail = DetailData::Error(format!("package not found: {name}")),
                }
                Task::none()
            }
            PackageSource::Group => {
                let installed_names = self.installed_names.clone();
                let resolved = self.alpm.as_ref().and_then(|alpm| {
                    find_groups(alpm, &name)
                        .into_iter()
                        .next()
                        .map(|(_, group)| {
                            group
                                .packages()
                                .iter()
                                .map(|p| GroupMember {
                                    name: p.name().to_string(),
                                    description: p.desc().map(|d| d.to_string()),
                                    installed: installed_names.contains(p.name()),
                                })
                                .collect::<Vec<_>>()
                        })
                });
                match resolved {
                    Some(members) => self.detail = DetailData::Group { name, members },
                    None => self.detail = DetailData::Error(format!("group not found: {name}")),
                }
                Task::none()
            }
            PackageSource::Aur => {
                let Some(aur_client) = self.aur_client.clone() else {
                    self.detail = DetailData::Error("aur unavailable".to_string());
                    return Task::none();
                };
                let local_index = self.local_index.clone();

                Task::stream(cosmic::iced::stream::channel(
                    8,
                    move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
                        let had_cache = match local_index.as_ref() {
                            Some(index) => match index.detail(&name) {
                                Ok(Some(info)) => {
                                    let _ = tx
                                        .send(
                                            crate::Message::Detail(DetailMessage::DetailReady {
                                                seq,
                                                pkg: Package::from(info),
                                            })
                                            .into(),
                                        )
                                        .await;
                                    true
                                }
                                Ok(None) => false,
                                Err(e) => {
                                    eprintln!("[pakajo] detail cache read failed: {e:#}");
                                    false
                                }
                            },
                            None => false,
                        };

                        let (otx, orx) = futures::channel::oneshot::channel();
                        let name_net = name.clone();
                        let index_net = local_index.clone();
                        std::thread::spawn(move || {
                            if had_cache {
                                std::thread::sleep(DETAIL_DEBOUNCE);
                            }
                            let fetched = aur_client.info(&name_net);
                            if let Ok(Some(ref info)) = fetched
                                && let Some(index) = index_net.as_ref()
                                && let Err(e) = index.put_detail(info)
                            {
                                eprintln!("[pakajo] detail put_detail failed: {e:#}");
                            }
                            let _ = otx.send(fetched);
                        });

                        let fetched = match orx.await {
                            Ok(r) => r,
                            Err(_) => return,
                        };
                        match fetched {
                            Ok(Some(info)) => {
                                let _ = tx
                                    .send(
                                        crate::Message::Detail(DetailMessage::DetailReady {
                                            seq,
                                            pkg: Package::from(info),
                                        })
                                        .into(),
                                    )
                                    .await;
                            }
                            Ok(None) => {
                                let _ = tx
                                    .send(
                                        crate::Message::Detail(DetailMessage::DetailFailed {
                                            seq,
                                            message: format!("package not found: {name}"),
                                        })
                                        .into(),
                                    )
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(
                                        crate::Message::Detail(DetailMessage::DetailFailed {
                                            seq,
                                            message: pakajo::search::friendly_search_error(&e),
                                        })
                                        .into(),
                                    )
                                    .await;
                            }
                        }
                    },
                ))
            }
        }
    }

    pub(crate) fn set_detail_pkg(&mut self, pkg: Package) {
        let name = pkg.name.clone();
        let installed = self
            .alpm
            .as_ref()
            .map(|a| pakajo::package::is_installed(a, &name))
            .unwrap_or(false);
        self.detail = DetailData::Ready { pkg, installed };
    }

    pub(crate) fn handle_detail(
        &mut self,
        message: DetailMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            DetailMessage::DetailReady { seq, pkg } => {
                if seq == self.detail_seq {
                    self.set_detail_pkg(pkg);
                }
                Task::none()
            }
            DetailMessage::DetailFailed { seq, message } => {
                if seq == self.detail_seq {
                    self.detail = DetailData::Error(message);
                }
                Task::none()
            }
        }
    }
}
