use cosmic::app::Task;
use cosmic::iced::core::text::Wrapping;
use cosmic::iced::stream::channel;
use cosmic::iced::{Alignment, Background, Border, Color, Length, Padding};
use cosmic::widget::{
    Column, Row, Space, button, container, flex_row, icon, mouse_area, responsive, row, scrollable,
    text, tooltip,
};
use futures::SinkExt as _;
use pakajo::package::{self, Package, PackageSource};
use pakajo::utils::{format_bytes, group_thousands};
use std::time::Duration;

use crate::Element;

pub(crate) fn selectable_optdeps(pkg: &Package, selection: &[String]) -> OptDepSelection {
    selection
        .iter()
        .filter_map(|name| {
            pkg.opt_dependencies
                .iter()
                .find(|dep| &dep.name == name && !dep.installed)
                .map(|dep| (name.clone(), dep.version.clone().unwrap_or_default()))
        })
        .collect()
}
use crate::PakajoCtx;
use crate::components::icons;
use crate::components::search::SearchMessage;
use crate::components::theme::{accent_color, destructive_color, muted_mono, muted_text as muted};
use crate::components::transaction::{OptDepSelection, TransactionMessage, TransactionRequest};
use cosmic::widget::divider;

pub const DETAIL_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Default)]
pub enum DetailData {
    #[default]
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
    Load { name: String, source: PackageSource },
    StartInstall,
    StartBatchInstall,
    StartRemove,
    DetailReady { seq: u64, pkg: Box<Package> },
    DetailFailed { seq: u64, message: String },
    ShowLoading { seq: u64 },
    ToggleOptDep(String),
    OptDepHover(Option<String>),
}

impl std::fmt::Debug for DetailMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load { name, source } => f
                .debug_struct("Load")
                .field("name", name)
                .field("source", source)
                .finish(),
            Self::StartInstall => f.debug_struct("StartInstall").finish(),
            Self::StartBatchInstall => f.debug_struct("StartBatchInstall").finish(),
            Self::StartRemove => f.debug_struct("StartRemove").finish(),
            Self::DetailReady { seq, .. } => {
                f.debug_struct("DetailReady").field("seq", seq).finish()
            }
            Self::DetailFailed { seq, message } => f
                .debug_struct("DetailFailed")
                .field("seq", seq)
                .field("message", message)
                .finish(),
            Self::ShowLoading { seq } => f.debug_struct("ShowLoading").field("seq", seq).finish(),
            Self::ToggleOptDep(name) => f.debug_struct("ToggleOptDep").field("name", name).finish(),
            Self::OptDepHover(name) => f.debug_struct("OptDepHover").field("name", name).finish(),
        }
    }
}

pub fn detail_view<'a>(
    detail: &'a DetailData,
    checking: Option<&'a str>,
    pending: bool,
    selection: &'a [String],
    disabled: bool,
    hovered: Option<&'a str>,
) -> Element<'a> {
    let content: Element<'a> = match detail {
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
        DetailData::Ready { pkg, installed } => render_package(
            pkg, *installed, checking, pending, selection, disabled, hovered,
        ),
    };

    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn render_group<'a>(name: &'a str, members: &'a [GroupMember]) -> Element<'a> {
    let mut col = Column::new()
        .spacing(12)
        .push(text::title3(name.to_string()))
        .push(divider::horizontal::default());

    for member in members {
        let mut row = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(crate::components::row_title(member.name.clone()))
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
                    border: Border {
                        radius: cosmic.corner_radii.radius_s.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        col = col.push(item);
    }

    container(col)
        .padding([
            cosmic::theme::spacing().space_s as f32,
            cosmic::theme::spacing().space_m as f32,
        ])
        .into()
}

fn render_package<'a>(
    pkg: &'a Package,
    installed: bool,
    checking: Option<&'a str>,
    pending: bool,
    selection: &'a [String],
    disabled: bool,
    hovered: Option<&'a str>,
) -> Element<'a> {
    let selected = selectable_optdeps(pkg, selection).len();
    let header = render_header(pkg, installed, checking, pending, selected);
    let details = render_details(pkg);
    let dependencies = render_dependencies(pkg);
    let opt_dependencies = render_opt_dependencies(pkg, selection, disabled, hovered, installed);

    let mut col = Column::new()
        .spacing(20)
        .push(header)
        .push(details)
        .push(dependencies);

    if !pkg.opt_dependencies.is_empty() {
        col = col.push(opt_dependencies);
    }

    container(col)
        .padding([
            cosmic::theme::spacing().space_s as f32,
            cosmic::theme::spacing().space_m as f32,
        ])
        .into()
}

fn render_header<'a>(
    pkg: &'a Package,
    installed: bool,
    checking: Option<&'a str>,
    pending: bool,
    selected: usize,
) -> Element<'a> {
    let formatted_name = if let Some(repo) = pkg.repo() {
        format!("{repo}/{}", pkg.name)
    } else {
        pkg.name.clone()
    };

    let (label, intent) = if installed {
        ("Remove", crate::Message::Detail(DetailMessage::StartRemove))
    } else {
        (
            "Install",
            crate::Message::Detail(DetailMessage::StartInstall),
        )
    };

    let busy = pending || checking == Some(pkg.name.as_str());
    let action: Element<'a> = if busy {
        button::standard("Loading...").into()
    } else if installed {
        button::destructive(label).on_press(intent).into()
    } else if selected > 0 {
        button::custom(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .height(cosmic::theme::spacing().space_l)
                .push(text("Install"))
                .push(install_count_badge(selected)),
        )
        .padding([0, cosmic::theme::spacing().space_s])
        .class(cosmic::theme::Button::Suggested)
        .on_press(intent)
        .into()
    } else {
        button::suggested(label).on_press(intent).into()
    };

    let mut actions = Row::new().spacing(8).align_y(Alignment::Center);

    if let Some(url) = &pkg.upstream_url {
        actions = actions.push(
            button::standard("Upstream")
                .trailing_icon(icons::external_link())
                .on_press(crate::Message::OpenUrl(url.clone())),
        );
    }

    actions = actions.push(action);

    let mut children: Vec<Element<'a>> = vec![
        row![
            text::title3(formatted_name),
            row![muted_mono(&pkg.version)].padding(Padding::ZERO.bottom(4))
        ]
        .spacing(8)
        .align_y(Alignment::End)
        .into(),
        Space::new().width(Length::Fill).into(),
    ];

    children.push(actions.into());

    let title_row = flex_row(children)
        .spacing(12)
        .align_items(Alignment::Center)
        .width(Length::Fill);

    let mut col = Column::new().spacing(12).push(title_row);

    if let Some(desc) = &pkg.description {
        col = col.push(muted(desc.clone()));
    }

    col = col.push(render_info_bar(pkg));
    col.into()
}

fn render_info_bar<'a>(pkg: &'a Package) -> Element<'a> {
    let mut info_row = Row::new().spacing(16).align_y(Alignment::Center);

    if !pkg.licenses.is_empty() {
        info_row = info_row.push(info_item(
            icon(icons::scale()).size(16).into(),
            pkg.licenses.join(", "),
        ));
    }

    if let Some(maintainer) = pkg.maintainer_name() {
        info_row = info_row.push(info_item(icon(icons::user()).size(16).into(), maintainer));
    }

    match &pkg.kind {
        pakajo::package::PackageKind::Repo(data) => {
            if let Some(arch) = data.architecture.as_ref() {
                info_row =
                    info_row.push(info_item(icon(icons::cpu()).size(16).into(), arch.clone()));
            }
            let size_str = format!(
                "{} / {}",
                format_bytes(data.download_size),
                format_bytes(data.installed_size)
            );
            info_row = info_row.push(tooltip(
                info_item(icon(icons::hard_drive()).size(16).into(), size_str),
                text(format!(
                    "{} B download, {} B installed",
                    group_thousands(data.download_size),
                    group_thousands(data.installed_size)
                )),
                tooltip::Position::FollowCursor,
            ));
        }
        pakajo::package::PackageKind::Aur(data) => {
            info_row = info_row.push(info_item(
                icon(icons::star()).size(16).into(),
                format!("+{} ({:.2})", data.num_votes, data.popularity),
            ));
        }
    }

    container(info_row.wrap())
        .padding([10.0, 14.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| card_style(theme))
        .into()
}

fn render_details<'a>(pkg: &'a Package) -> Element<'a> {
    responsive(move |size| {
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

        if size.width < 480.0 {
            Column::new()
                .spacing(16)
                .push(provides_col)
                .push(conflicts_col)
                .into()
        } else {
            Row::new()
                .spacing(16)
                .push(container(provides_col).width(Length::Fill))
                .push(container(conflicts_col).width(Length::Fill))
                .into()
        }
    })
    .width(Length::Fill)
    .into()
}

fn render_badge_section<'a>(
    title: String,
    items: &'a [String],
    color_fn: fn(&cosmic::Theme) -> Color,
) -> Element<'a> {
    let mut col = Column::new().spacing(8).push(section_header(title));

    if items.is_empty() {
        col = col.push(muted("None"));
    } else {
        let mut row = Row::new().spacing(6);
        for item in items {
            row = row.push(badge_tag(item.clone(), color_fn));
        }
        col = col.push(row.wrap());
    }

    col.into()
}

fn render_dependencies<'a>(pkg: &'a Package) -> Element<'a> {
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

const OPTDEP_BOX_SIZE: f32 = 16.0;
const OPTDEP_HIT_PADDING: f32 = 8.0;

fn optdep_box_style(
    theme: &cosmic::Theme,
    checked: bool,
    lit: bool,
    dimmed: bool,
) -> container::Style {
    let cosmic = theme.cosmic();
    let accent = accent_color(theme);
    let mut fill = if checked {
        accent
    } else {
        cosmic.background(false).small_widget.into()
    };
    if lit && !checked {
        fill = cosmic::cosmic_theme::composite::over(
            Color {
                a: 0.1,
                ..cosmic.palette.neutral_0.into()
            },
            fill,
        )
        .into();
    }
    let alpha = if dimmed { 0.5 } else { 1.0 };
    fill.a *= alpha;
    let outline = if checked || lit {
        accent
    } else {
        cosmic.palette.neutral_8.into()
    };
    container::Style {
        background: Some(Background::Color(fill)),
        border: Border {
            radius: cosmic.corner_radii.radius_xs.into(),
            width: if checked { 0.0 } else { 1.0 },
            color: Color {
                a: alpha,
                ..outline
            },
        },
        ..Default::default()
    }
}

fn optdep_zone_style(theme: &cosmic::Theme, alpha: f32) -> cosmic::widget::button::Style {
    let background = (alpha > 0.0).then(|| {
        Background::Color(Color {
            a: alpha,
            ..accent_color(theme)
        })
    });
    cosmic::widget::button::Style {
        background,
        border_radius: theme.cosmic().corner_radii.radius_xs.into(),
        ..Default::default()
    }
}

fn optdep_checkbox<'a>(name: &'a str, checked: bool, disabled: bool, hovered: bool) -> Element<'a> {
    let glyph: Element<'a> = if checked {
        icon(icons::check())
            .size(12)
            .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(move |t| {
                cosmic::widget::svg::Style {
                    color: Some(Color {
                        a: if disabled { 0.5 } else { 1.0 },
                        ..Color::from(t.cosmic().accent.on)
                    }),
                }
            })))
            .into()
    } else {
        Space::new().into()
    };
    let hit_zone = button::custom(
        container(glyph)
            .center(OPTDEP_BOX_SIZE)
            .style(move |t| optdep_box_style(t, checked, hovered && !disabled, disabled)),
    )
    .padding(OPTDEP_HIT_PADDING)
    .class(cosmic::theme::Button::Transparent)
    .on_press_maybe(
        (!disabled).then(|| crate::Message::Detail(DetailMessage::ToggleOptDep(name.to_string()))),
    );
    mouse_area(hit_zone)
        .on_enter(crate::Message::Detail(DetailMessage::OptDepHover(Some(
            name.to_string(),
        ))))
        .on_exit(crate::Message::Detail(DetailMessage::OptDepHover(None)))
        .into()
}

fn optdep_installed_marker<'a>() -> Element<'a> {
    container(
        icon(icons::circle_check())
            .size(OPTDEP_BOX_SIZE as u16)
            .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                    color: Some(theme.cosmic().success.base.into()),
                },
            ))),
    )
    .padding(OPTDEP_HIT_PADDING)
    .into()
}

fn optdep_install_label<'a>(label: String) -> Element<'a> {
    Row::new()
        .spacing(4)
        .align_y(Alignment::Center)
        .push(
            icon(icons::download())
                .size(12)
                .class(cosmic::theme::Svg::Custom(std::rc::Rc::new(
                    |theme: &cosmic::Theme| cosmic::widget::svg::Style {
                        color: Some(accent_color(theme)),
                    },
                ))),
        )
        .push(
            text(label)
                .size(12.0)
                .line_height(cosmic::iced::core::text::LineHeight::Absolute(17.0.into()))
                .font(cosmic::font::default())
                .class(cosmic::theme::Text::Accent),
        )
        .into()
}

fn render_opt_dependencies<'a>(
    pkg: &'a Package,
    selection: &'a [String],
    disabled: bool,
    hovered: Option<&'a str>,
    installed: bool,
) -> Element<'a> {
    let selected = selectable_optdeps(pkg, selection).len();
    let install_link = (installed && selected > 0).then(|| {
        button::custom(optdep_install_label(format!("Install {selected}")))
            .class(cosmic::theme::Button::Custom {
                active: Box::new(|_, theme| optdep_zone_style(theme, 0.0)),
                disabled: Box::new(|theme| optdep_zone_style(theme, 0.0)),
                hovered: Box::new(|_, theme| optdep_zone_style(theme, 0.10)),
                pressed: Box::new(|_, theme| optdep_zone_style(theme, 0.16)),
            })
            .padding([2.0, 8.0])
            .on_press_maybe(
                (!disabled).then(|| crate::Message::Detail(DetailMessage::StartBatchInstall)),
            )
    });
    let header: Element<'a> = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(text::heading(format!(
                    "Optional Dependencies ({})",
                    pkg.opt_dependencies.len()
                )))
                .push(Space::new().width(Length::Fill))
                .push_maybe(install_link),
        )
        .push(divider::horizontal::default())
        .into();
    let col = Column::new().spacing(8).push(header);

    let mut list = Column::new().spacing(6);

    for dep in &pkg.opt_dependencies {
        let name: Element<'a> = button::custom(crate::components::row_title(dep.name.clone()))
            .class(cosmic::theme::Button::ListItem([0.0; 4]))
            .padding(0)
            .on_press(crate::Message::Search(SearchMessage::QueryChanged(
                dep.name.clone(),
            )))
            .into();
        let row = Row::new()
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .push_maybe((!dep.installed).then(|| {
                optdep_checkbox(
                    &dep.name,
                    selection.contains(&dep.name),
                    disabled,
                    hovered == Some(dep.name.as_str()),
                )
            }))
            .push_maybe(dep.installed.then(optdep_installed_marker))
            .push(name)
            .push_maybe(
                dep.version
                    .is_some()
                    .then(|| Space::new().width(Length::Fixed(8.0))),
            )
            .push_maybe(dep.version.as_deref().map(muted_mono))
            .push(Space::new().width(Length::Fill))
            .push_maybe(dep.reason.as_ref().map(|r| muted(r.clone())))
            .push(Space::new().width(Length::Fixed(16.0)));

        let item = container(row)
            .padding([0.0, 12.0, 0.0, 0.0])
            .width(Length::Fill)
            .style(|theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: Some(Background::Color(Color::from(
                        cosmic.background(false).small_widget,
                    ))),
                    border: Border {
                        radius: cosmic.corner_radii.radius_s.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        list = list.push(item);
    }

    col.push(list).into()
}

fn info_item<'a>(icon: Element<'a>, label: String) -> Element<'a> {
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(icon)
        .push(muted(label))
        .into()
}

fn install_count_badge<'a>(selected: usize) -> Element<'a> {
    container(text::caption(format!("+{selected}")))
        .padding([2.0, 8.0])
        .style(|theme: &cosmic::Theme| {
            let on = Color::from(theme.cosmic().accent.on);
            container::Style {
                text_color: Some(on),
                background: Some(Background::Color(Color { a: 0.22, ..on })),
                border: Border {
                    radius: theme.cosmic().corner_radii.radius_xl.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn badge_tag<'a>(label: String, color_fn: fn(&cosmic::Theme) -> Color) -> Element<'a> {
    container(text::caption(label).wrapping(Wrapping::None))
        .padding([4.0, 8.0])
        .style(move |theme: &cosmic::Theme| {
            let c = color_fn(theme);
            container::Style {
                background: Some(Background::Color(Color { a: 0.12, ..c })),
                text_color: Some(c),
                border: Border {
                    radius: theme.cosmic().corner_radii.radius_xs.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn secondary_tag<'a>(label: String) -> Element<'a> {
    container(text::caption(label))
        .padding([4.0, 8.0])
        .style(|theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            let bg = Color::from(cosmic.background(false).small_widget);
            let on = Color::from(cosmic.background(false).on);
            container::Style {
                background: Some(Background::Color(bg)),
                text_color: Some(on),
                border: Border {
                    radius: cosmic.corner_radii.radius_xs.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn section_header<'a>(title: String) -> Element<'a> {
    Column::new()
        .spacing(4)
        .push(text::heading(title))
        .push(divider::horizontal::default())
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
        border: Border {
            radius: cosmic.corner_radii.radius_s.into(),
            width: 1.0,
            color: border,
        },
        ..Default::default()
    }
}

pub(crate) fn revalidate_selection(
    prev_name: Option<&str>,
    new_pkg: &Package,
    selection: &[String],
) -> Vec<String> {
    if prev_name != Some(new_pkg.name.as_str()) {
        return Vec::new();
    }
    selection
        .iter()
        .filter(|name| {
            new_pkg
                .opt_dependencies
                .iter()
                .any(|dep| &dep.name == *name && !dep.installed)
        })
        .cloned()
        .collect()
}

#[derive(Default)]
pub struct DetailPane {
    pub(crate) data: DetailData,
    pub(crate) seq: u64,
    pub(crate) pending: Option<u64>,
    pub(crate) selected_optdeps: Vec<String>,
    pub(crate) pkg_name: Option<String>,
    pub(crate) optdep_hover: Option<String>,
}

impl DetailPane {
    pub fn update(
        &mut self,
        message: DetailMessage,
        ctx: &PakajoCtx,
        busy: bool,
    ) -> Task<crate::Message> {
        match message {
            DetailMessage::Load { name, source } => self.load_detail(name, source, ctx),
            DetailMessage::StartInstall if !self.blocked(busy) => {
                let Some((name, source, with_deps)) = self.install_target() else {
                    return Task::none();
                };
                if !with_deps.is_empty() {
                    self.selected_optdeps.clear();
                }
                self.begin(TransactionRequest::Install {
                    name,
                    source,
                    with_deps,
                })
            }
            DetailMessage::StartBatchInstall if !self.blocked(busy) => {
                let Some((_, _, with_deps)) = self.install_target() else {
                    return Task::none();
                };
                if with_deps.is_empty() {
                    return Task::none();
                }
                self.selected_optdeps.clear();
                self.begin(TransactionRequest::BatchInstall(with_deps))
            }
            DetailMessage::StartRemove if !self.blocked(busy) => {
                let DetailData::Ready { pkg, .. } = &self.data else {
                    return Task::none();
                };
                self.begin(TransactionRequest::Remove {
                    name: pkg.name.clone(),
                    source: pkg.source(),
                    description: pkg.description.clone(),
                    repo: pkg.repo().map(str::to_string),
                })
            }
            DetailMessage::StartInstall
            | DetailMessage::StartBatchInstall
            | DetailMessage::StartRemove => Task::none(),
            DetailMessage::DetailReady { seq, pkg } => self.ready(seq, *pkg, ctx),
            DetailMessage::DetailFailed { seq, message } => {
                if seq == self.seq {
                    self.pending = None;
                    self.data = DetailData::Error(message);
                }
                Task::none()
            }
            DetailMessage::ShowLoading { seq } => {
                if seq == self.seq && self.pending == Some(seq) {
                    self.data = DetailData::Loading;
                }
                Task::none()
            }
            DetailMessage::OptDepHover(name) => {
                self.optdep_hover = name;
                Task::none()
            }
            DetailMessage::ToggleOptDep(name) => {
                if busy {
                    return Task::none();
                }
                let DetailData::Ready { pkg, .. } = &self.data else {
                    return Task::none();
                };
                if !pkg
                    .opt_dependencies
                    .iter()
                    .any(|d| d.name == name && !d.installed)
                {
                    return Task::none();
                }
                if let Some(index) = self.selected_optdeps.iter().position(|n| *n == name) {
                    self.selected_optdeps.remove(index);
                } else {
                    self.selected_optdeps.push(name);
                }
                Task::none()
            }
        }
    }

    fn ready(&mut self, seq: u64, pkg: Package, ctx: &PakajoCtx) -> Task<crate::Message> {
        if seq == self.seq {
            self.pending = None;
            self.set_detail_pkg(pkg, ctx);
        }
        Task::none()
    }

    fn blocked(&self, busy: bool) -> bool {
        busy || self.pending.is_some()
    }

    fn install_target(
        &self,
    ) -> Option<(
        String,
        PackageSource,
        crate::components::transaction::OptDepSelection,
    )> {
        let DetailData::Ready { pkg, .. } = &self.data else {
            return None;
        };
        Some((
            pkg.name.clone(),
            pkg.source(),
            selectable_optdeps(pkg, &self.selected_optdeps),
        ))
    }

    fn begin(&self, request: TransactionRequest) -> Task<crate::Message> {
        Task::done(cosmic::Action::App(crate::Message::Transaction(
            TransactionMessage::Begin(request),
        )))
    }

    pub fn refresh_installed(&mut self, ctx: &PakajoCtx) {
        let pkg = match &self.data {
            DetailData::Ready { pkg, .. } => pkg.as_ref().clone(),
            _ => return,
        };
        self.set_detail_pkg(pkg, ctx);
    }

    pub(crate) fn load_detail(
        &mut self,
        name: String,
        source: PackageSource,
        ctx: &PakajoCtx,
    ) -> Task<crate::Message> {
        self.seq = self.seq.wrapping_add(1);
        self.pending = None;
        let seq = self.seq;
        if matches!(self.data, DetailData::None) {
            self.data = DetailData::Pending;
        }
        match source {
            PackageSource::Repo => {
                let resolved = ctx
                    .alpm
                    .as_ref()
                    .and_then(|alpm| package::find(alpm, &name));
                match resolved {
                    Some(pkg) => self.set_detail_pkg(pkg, ctx),
                    None => self.data = DetailData::Error(format!("package not found: {name}")),
                }
                Task::none()
            }
            PackageSource::Group => {
                let installed_names = ctx.installed_names.clone();
                let resolved = ctx.alpm.as_ref().and_then(|alpm| {
                    package::find_groups(alpm, &name)
                        .into_iter()
                        .next()
                        .map(|group| {
                            group
                                .members
                                .iter()
                                .map(|p| GroupMember {
                                    name: p.name.clone(),
                                    description: p.description.clone(),
                                    installed: installed_names.contains(&p.name),
                                })
                                .collect::<Vec<_>>()
                        })
                });
                match resolved {
                    Some(members) => self.data = DetailData::Group { name, members },
                    None => self.data = DetailData::Error(format!("group not found: {name}")),
                }
                Task::none()
            }
            PackageSource::Aur => {
                let Some(aur_client) = ctx.aur_client.clone() else {
                    self.data = DetailData::Error("aur unavailable".to_string());
                    return Task::none();
                };
                let db = ctx.db.clone();
                self.pending = Some(seq);
                Task::batch([
                    Task::stream(channel(
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

    pub(crate) fn set_detail_pkg(&mut self, mut pkg: Package, ctx: &PakajoCtx) {
        let name = pkg.name.clone();
        let installed = ctx
            .alpm
            .as_ref()
            .map(|a| pakajo::package::is_installed(a, &name))
            .unwrap_or(false);
        match ctx.alpm.as_ref() {
            Some(alpm) => {
                for dep in &mut pkg.opt_dependencies {
                    dep.installed = package::opt_dep_installed(alpm, dep);
                }
            }
            None => {
                for dep in &mut pkg.opt_dependencies {
                    dep.installed = false;
                }
            }
        }
        self.selected_optdeps =
            revalidate_selection(self.pkg_name.as_deref(), &pkg, &self.selected_optdeps);
        self.pkg_name = Some(name);
        self.data = DetailData::Ready {
            pkg: Box::new(pkg),
            installed,
        };
    }
}

fn show_loading_after_debounce(seq: u64) -> Task<crate::Message> {
    Task::stream(channel(
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
