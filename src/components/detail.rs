use cosmic::app::Task;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};
use futures::SinkExt as _;
use pakajo::package::{Package, PackageSource};
use pakajo::pacman::{find_groups, find_pkg};
use pakajo::utils::format_bytes;
use std::time::Duration;

use crate::components::icons;
use crate::components::transaction::TransactionMessage;

pub const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub enum DetailData {
    None,
    Pending,
    Loading,
    Ready {
        pkg: Box<Package>,
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
    DetailReady { seq: u64, pkg: Box<Package> },
    DetailFailed { seq: u64, message: String },
    ShowLoading { seq: u64 },
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
            Self::ShowLoading { seq } => f.debug_struct("ShowLoading").field("seq", seq).finish(),
        }
    }
}

pub fn detail_view<'a>(
    detail: &'a DetailData,
    checking: Option<&'a str>,
    pending: bool,
) -> cosmic::Element<'a, crate::Message> {
    let content: cosmic::Element<'a, crate::Message> = match detail {
        DetailData::None => container(muted("Select a package"))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
        DetailData::Pending => Space::new().width(Length::Fill).height(Length::Fill).into(),
        DetailData::Loading => container(text("Loading..."))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
        DetailData::Error(msg) => container(text(msg.clone()))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
        DetailData::Group { name, members } => render_group(name, members),
        DetailData::Ready { pkg, installed } => render_package(pkg, *installed, checking, pending),
    };

    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn render_group<'a>(
    name: &'a str,
    members: &'a [GroupMember],
) -> cosmic::Element<'a, crate::Message> {
    let mut col = Column::new()
        .spacing(12)
        .push(text(name.to_string()).font(cosmic::font::bold()).size(22.0))
        .push(divider());

    for member in members {
        let mut row = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(text(member.name.clone()).font(cosmic::font::semibold()))
            .push_maybe(member.description.as_ref().map(|d| muted(d.clone())));

        if member.installed {
            row = row
                .push(Space::new().width(Length::Fill))
                .push(muted("installed"));
        }

        let item = container(row)
            .padding([8.0, 12.0])
            .width(Length::Fill)
            .style(|theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: Some(Background::Color(Color::from(
                        cosmic.background(false).small_widget,
                    ))),
                    border: cosmic::iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        col = col.push(item);
    }

    container(col).padding(16.0).into()
}

fn render_package<'a>(
    pkg: &'a Package,
    installed: bool,
    checking: Option<&'a str>,
    pending: bool,
) -> cosmic::Element<'a, crate::Message> {
    let header = render_header(pkg, installed, checking, pending);
    let details = render_details(pkg);
    let dependencies = render_dependencies(pkg);
    let opt_dependencies = render_opt_dependencies(pkg);

    let mut col = Column::new()
        .spacing(20)
        .push(header)
        .push(details)
        .push(dependencies);

    if !pkg.opt_dependencies.is_empty() {
        col = col.push(opt_dependencies);
    }

    container(col).padding(16.0).into()
}

fn render_header<'a>(
    pkg: &'a Package,
    installed: bool,
    checking: Option<&'a str>,
    pending: bool,
) -> cosmic::Element<'a, crate::Message> {
    let formatted_name = if let Some(repo) = pkg.repo.as_ref() {
        format!("{repo}/{}", pkg.name)
    } else {
        pkg.name.clone()
    };

    let (label, intent) = if installed {
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

    let busy = pending || checking == Some(pkg.name.as_str());
    let action = if busy {
        button::custom(text("Loading..."))
    } else if installed {
        button::custom(text(label))
            .class(cosmic::theme::Button::Destructive)
            .on_press(intent)
    } else {
        button::custom(text(label))
            .class(cosmic::theme::Button::Suggested)
            .on_press(intent)
    };

    let title_row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(text(formatted_name).font(cosmic::font::bold()).size(24.0))
        .push(muted(&pkg.version))
        .push(Space::new().width(Length::Fill))
        .push(action);

    let mut col = Column::new().spacing(12).push(title_row);

    if let Some(desc) = &pkg.description {
        col = col.push(muted(desc.clone()));
    }

    col = col.push(render_info_bar(pkg));
    col.into()
}

fn render_info_bar<'a>(pkg: &'a Package) -> cosmic::Element<'a, crate::Message> {
    let mut info_row = Row::new().spacing(16).align_y(Alignment::Center);

    if !pkg.licenses.is_empty() {
        info_row = info_row.push(info_item(
            icons::scale().width(16).height(16).into(),
            pkg.licenses.join(", "),
        ));
    }

    if let Some(maintainer) = pkg.maintainer_name() {
        info_row = info_row.push(info_item(
            icons::user().width(16).height(16).into(),
            maintainer,
        ));
    }

    if let Some(arch) = &pkg.architecture {
        info_row = info_row.push(info_item(
            icons::cpu().width(16).height(16).into(),
            arch.clone(),
        ));
    }

    if let Some((votes, popularity)) = pkg.num_votes.zip(pkg.popularity) {
        info_row = info_row.push(info_item(
            icons::star().width(16).height(16).into(),
            format!("+{votes} ({popularity:.2})"),
        ));
    }

    if let Some((download, installed)) = pkg.download_size.zip(pkg.installed_size) {
        let size_str = format!("{} / {}", format_bytes(download), format_bytes(installed));
        info_row = info_row.push(info_item(
            icons::hard_drive().width(16).height(16).into(),
            size_str,
        ));
    }

    container(info_row)
        .padding([10.0, 14.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| card_style(theme))
        .into()
}

fn render_details<'a>(pkg: &'a Package) -> cosmic::Element<'a, crate::Message> {
    let provides_col = render_badge_section(
        format!("Provides ({})", pkg.provides.len()),
        &pkg.provides,
        accent_color,
    );

    let conflicts_col = render_badge_section(
        format!("Conflicts ({})", pkg.conflicts.len()),
        &pkg.conflicts,
        destructive_color,
    );

    Row::new()
        .spacing(16)
        .width(Length::Fill)
        .push(container(provides_col).width(Length::Fill))
        .push(container(conflicts_col).width(Length::Fill))
        .into()
}

fn render_badge_section<'a>(
    title: String,
    items: &'a [String],
    color_fn: fn(&cosmic::Theme) -> Color,
) -> cosmic::Element<'a, crate::Message> {
    let mut col = Column::new().spacing(8).push(section_header(title));

    if items.is_empty() {
        col = col.push(muted("None"));
    } else {
        let mut row = Row::new().spacing(6);
        for item in items {
            row = row.push(badge_tag(item.clone(), color_fn));
        }
        col = col.push(row);
    }

    col.into()
}

fn render_dependencies<'a>(pkg: &'a Package) -> cosmic::Element<'a, crate::Message> {
    let mut col = Column::new().spacing(8).push(section_header(format!(
        "Dependencies ({})",
        pkg.dependencies.len()
    )));

    if pkg.dependencies.is_empty() {
        col = col.push(muted("None"));
    } else {
        let mut row = Row::new().spacing(6);
        for dep in &pkg.dependencies {
            row = row.push(secondary_tag(dep.clone()));
        }
        col = col.push(row.wrap());
    }

    col.into()
}

fn render_opt_dependencies<'a>(pkg: &'a Package) -> cosmic::Element<'a, crate::Message> {
    let col = Column::new().spacing(8).push(section_header(format!(
        "Optional Dependencies ({})",
        pkg.opt_dependencies.len()
    )));

    let mut list = Column::new().spacing(6);

    for dep in &pkg.opt_dependencies {
        let row = Row::new()
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .push(text(dep.name.clone()).font(cosmic::font::semibold()))
            .push(Space::new().width(Length::Fill))
            .push_maybe(dep.reason.as_ref().map(|r| muted(r.clone())));

        let item = container(row)
            .padding([8.0, 12.0])
            .width(Length::Fill)
            .style(|theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: Some(Background::Color(Color::from(
                        cosmic.background(false).small_widget,
                    ))),
                    border: cosmic::iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        list = list.push(item);
    }

    col.push(list).into()
}

fn info_item<'a>(
    icon: cosmic::Element<'a, crate::Message>,
    label: String,
) -> cosmic::Element<'a, crate::Message> {
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(icon)
        .push(muted(label))
        .into()
}

fn badge_tag<'a>(
    label: String,
    color_fn: fn(&cosmic::Theme) -> Color,
) -> cosmic::Element<'a, crate::Message> {
    container(text(label).size(13.0))
        .padding([4.0, 8.0])
        .style(move |theme: &cosmic::Theme| {
            let c = color_fn(theme);
            container::Style {
                background: Some(Background::Color(Color { a: 0.12, ..c })),
                text_color: Some(c),
                border: cosmic::iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn secondary_tag<'a>(label: String) -> cosmic::Element<'a, crate::Message> {
    container(text(label).size(13.0))
        .padding([4.0, 8.0])
        .style(|theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            let bg = Color::from(cosmic.background(false).small_widget);
            let on = Color::from(cosmic.background(false).on);
            container::Style {
                background: Some(Background::Color(bg)),
                text_color: Some(on),
                border: cosmic::iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn section_header<'a>(title: String) -> cosmic::Element<'a, crate::Message> {
    Column::new()
        .spacing(4)
        .push(text(title).font(cosmic::font::semibold()).size(14.0))
        .push(divider())
        .into()
}

fn divider<'a>() -> cosmic::Element<'a, crate::Message> {
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color::from(
                theme.cosmic().background(false).divider,
            ))),
            ..Default::default()
        })
        .into()
}

fn card_style(theme: &cosmic::Theme) -> container::Style {
    let cosmic = theme.cosmic();
    let bg = Color::from(cosmic.background(false).small_widget);
    let border = Color::from(cosmic.background(false).divider);
    let on = Color::from(cosmic.background(false).on);
    container::Style {
        text_color: Some(on),
        background: Some(Background::Color(bg)),
        border: cosmic::iced::Border {
            radius: 8.0.into(),
            width: 1.0,
            color: border,
        },
        ..Default::default()
    }
}

fn muted<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::Element<'a, crate::Message> {
    text(content)
        .class(cosmic::theme::Text::Custom(|theme| {
            let mut on = Color::from(theme.cosmic().background(false).on);
            on.a = 0.6;
            cosmic::iced::widget::text::Style {
                color: Some(on),
                ..Default::default()
            }
        }))
        .into()
}

fn accent_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().accent.base)
}

fn destructive_color(theme: &cosmic::Theme) -> Color {
    Color::from(theme.cosmic().destructive.base)
}

impl crate::PakajoApp {
    pub(crate) fn load_detail(
        &mut self,
        name: String,
        source: PackageSource,
    ) -> cosmic::app::Task<crate::Message> {
        self.detail_seq = self.detail_seq.wrapping_add(1);
        self.detail_pending = None;
        let seq = self.detail_seq;
        if matches!(self.detail, DetailData::None) {
            self.detail = DetailData::Pending;
        }
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
                let db = self.db.clone();
                self.detail_pending = Some(seq);
                Task::batch([
                    Task::stream(cosmic::iced::stream::channel(
                        8,
                        move |mut tx: futures::channel::mpsc::Sender<
                            cosmic::Action<crate::Message>,
                        >| async move {
                            let had_cache = match db.as_ref() {
                                Some(index) => match index.detail(&name) {
                                    Ok(Some(info)) => {
                                        let _ = tx
                                            .send(
                                                crate::Message::Detail(
                                                    DetailMessage::DetailReady {
                                                        seq,
                                                        pkg: Box::new(Package::from(info)),
                                                    },
                                                )
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
                            let index_net = db.clone();
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
                                                pkg: Box::new(Package::from(info)),
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
                    )),
                    show_loading_after_debounce(seq),
                ])
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
        self.detail = DetailData::Ready {
            pkg: Box::new(pkg),
            installed,
        };
    }

    pub(crate) fn handle_detail(
        &mut self,
        message: DetailMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            DetailMessage::DetailReady { seq, pkg } => {
                if seq == self.detail_seq {
                    self.detail_pending = None;
                    self.set_detail_pkg(*pkg);
                }
                Task::none()
            }
            DetailMessage::DetailFailed { seq, message } => {
                if seq == self.detail_seq {
                    self.detail_pending = None;
                    self.detail = DetailData::Error(message);
                }
                Task::none()
            }
            DetailMessage::ShowLoading { seq } => {
                if seq == self.detail_seq && self.detail_pending == Some(seq) {
                    self.detail = DetailData::Loading;
                }
                Task::none()
            }
        }
    }
}

fn show_loading_after_debounce(seq: u64) -> cosmic::app::Task<crate::Message> {
    Task::stream(cosmic::iced::stream::channel(
        1,
        move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
            let (fire_tx, fire_rx) = futures::channel::oneshot::channel::<()>();
            std::thread::spawn(move || {
                std::thread::sleep(DETAIL_DEBOUNCE);
                let _ = fire_tx.send(());
            });
            if fire_rx.await.is_ok() {
                let _ = tx
                    .send(crate::Message::Detail(DetailMessage::ShowLoading { seq }).into())
                    .await;
            }
        },
    ))
}
