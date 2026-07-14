use crate::{
    icon::PakajoIcon,
    package::Package,
    root::{InstallProgress, PakajoRoot},
    utils::format_bytes,
};
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    spinner::Spinner,
    tooltip::Tooltip,
    *,
};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
pub enum SizeTooltipTarget {
    Download,
    Installed,
}

impl SizeTooltipTarget {
    fn label(&self) -> &'static str {
        match self {
            Self::Download => "Download size",
            Self::Installed => "Installed size",
        }
    }
}

pub struct PackageDetail {
    pub pkg: Package,
    pub installed: bool,
    pub active_tooltip: Option<SizeTooltipTarget>,
    pub root: WeakEntity<PakajoRoot>,
    pub install_progress: InstallProgress,
}

impl PackageDetail {
    fn format_name(&self) -> String {
        if let Some(repo) = self.pkg.repo.as_ref() {
            format!("{}/{}", repo, self.pkg.name)
        } else {
            self.pkg.name.clone()
        }
    }

    fn sized_value(
        &self,
        entity: &Entity<PackageDetail>,
        target: SizeTooltipTarget,
        value: i64,
    ) -> AnyElement {
        let active_tooltip = self.active_tooltip;
        let tooltip_text = format!("{}: {}", target.label(), format_bytes(value));

        div()
            .child(format_bytes(value))
            .id(target.label())
            .on_hover({
                let entity = entity.clone();
                move |is_hovered, _window, cx| {
                    entity.update(cx, |this, cx| {
                        this.active_tooltip = (*is_hovered).then_some(target);
                        cx.notify();
                    });
                }
            })
            .on_prepaint({
                let entity = entity.clone();
                move |bounds, window, cx| {
                    if active_tooltip != Some(target) {
                        return;
                    }
                    let view = Tooltip::new(tooltip_text.clone()).build(window, cx);
                    let mut measure = view.clone().into_any();
                    let size = measure.layout_as_root(AvailableSpace::min_size(), window, cx);
                    window.set_tooltip(AnyTooltip {
                        view,
                        mouse_position: point(
                            bounds.center().x - size.width / 2.,
                            bounds.origin.y - rems(0.75).to_pixels(window.rem_size()),
                        ),
                        check_visible_and_update: Rc::new(
                            |_: Bounds<Pixels>, _: &mut Window, _: &mut App| true,
                        ),
                    });
                    entity.update(cx, |_, cx| cx.notify());
                }
            })
            .into_any_element()
    }

    fn info_bar(&self, cx: &App, entity: Entity<PackageDetail>) -> impl IntoElement {
        fn info_item(cx: &App, icon: PakajoIcon, label: String) -> impl IntoElement {
            div()
                .h_flex()
                .gap_2()
                .child(Icon::new(icon).text_color(cx.theme().muted_foreground))
                .child(label)
        }

        let sizes =
            self.pkg
                .download_size
                .zip(self.pkg.installed_size)
                .map(|(download, installed)| {
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Icon::new(PakajoIcon::HardDrive)
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(self.sized_value(&entity, SizeTooltipTarget::Download, download))
                        .child(div().text_color(cx.theme().muted_foreground).child("/"))
                        .child(self.sized_value(&entity, SizeTooltipTarget::Installed, installed))
                });

        div()
            .h_flex()
            .gap_6()
            .mt_4()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted)
            .py_3()
            .px_4()
            .child(info_item(
                cx,
                PakajoIcon::Scale,
                self.pkg.licenses.join(", "),
            ))
            .children(
                self.pkg
                    .maintainer_name()
                    .map(|name| info_item(cx, PakajoIcon::User, name)),
            )
            .children(
                self.pkg
                    .architecture
                    .as_ref()
                    .map(|arch| info_item(cx, PakajoIcon::Cpu, arch.clone())),
            )
            .children(
                self.pkg
                    .num_votes
                    .zip(self.pkg.popularity)
                    .map(|(votes, popularity)| {
                        info_item(
                            cx,
                            PakajoIcon::Star,
                            format!("+{} ({:.2})", votes, popularity),
                        )
                    }),
            )
            .children(sizes)
    }

    fn header(
        &self,
        window: &Window,
        cx: &App,
        entity: Entity<PackageDetail>,
    ) -> impl IntoElement {
        fn baseline_from_top(window: &Window, text: &str, rems: f32) -> Pixels {
            let font_size = gpui::rems(rems).to_pixels(window.rem_size());
            let line_height = window.pixel_snap(font_size);
            let layout = window.text_system().layout_line(
                text,
                font_size,
                &[window.text_style().to_run(text.len())],
                None,
            );
            (line_height - layout.ascent - layout.descent) / 2. + layout.ascent
        }

        fn title(
            cx: &App,
            name: String,
            version: String,
            installed: bool,
            install_progress: InstallProgress,
            root: WeakEntity<PakajoRoot>,
            window: &Window,
        ) -> impl IntoElement {
            let name_size = 2.0;
            let version_size = 1.5;
            let pad = baseline_from_top(window, &name, name_size)
                - baseline_from_top(window, &version, version_size);

            let (label, disabled) = match &install_progress {
                InstallProgress::Idle if installed => ("Installed", true),
                InstallProgress::Running => ("Installing…", true),
                InstallProgress::Idle | InstallProgress::Failed(_) => ("Install", false),
            };

            let root_for_click = root.clone();
            let mut install_button = Button::new("install-button")
                .label(label)
                .disabled(disabled)
                .rounded_none()
                .large()
                .on_click(move |_, _, cx| {
                    if let Some(root) = root_for_click.upgrade() {
                        root.update(cx, |root, cx| root.start_install(cx));
                    }
                });
            if !installed && !matches!(install_progress, InstallProgress::Running) {
                install_button = install_button.primary();
            }

            let aside: Option<AnyElement> = match &install_progress {
                InstallProgress::Running => Some(Spinner::new().into_any_element()),
                InstallProgress::Failed(message) => Some(
                    div()
                        .text_color(cx.theme().danger)
                        .text_size(rems(0.875))
                        .child(message.clone())
                        .into_any_element(),
                ),
                InstallProgress::Idle => None,
            };

            div()
                .h_flex()
                .gap_4()
                .line_height(relative(1.0))
                .items_start()
                .child(div().text_size(rems(name_size)).child(name))
                .child(
                    div()
                        .text_size(rems(version_size))
                        .text_color(cx.theme().muted_foreground)
                        .mt(pad)
                        .child(version),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .items_center()
                        .ml_auto()
                        .child(install_button)
                        .children(aside),
                )
        }

        div()
            .v_flex()
            .child(title(
                cx,
                self.format_name(),
                self.pkg.version.clone(),
                self.installed,
                self.install_progress.clone(),
                self.root.clone(),
                window,
            ))
            .children(
                self.pkg
                    .description
                    .clone()
                    .map(|desc| div().text_color(cx.theme().muted_foreground).child(desc)),
            )
            .child(self.info_bar(cx, entity))
    }

    fn details(&self, cx: &App) -> impl IntoElement {
        fn section(cx: &App, title: String, items: &[String], color: Hsla) -> impl IntoElement {
            div()
                .v_flex()
                .w_full()
                .child(div().font_semibold().child(title))
                .child(div().h_px().w_full().mt_1().mb_3().bg(cx.theme().border))
                .child(if !items.is_empty() {
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .children(items.iter().map(|x| {
                            div()
                                .child(x.clone())
                                // TODO: is this a good idea?
                                .text_color(color.saturation(0.6))
                                .bg(color.opacity(0.1))
                                .line_height(relative(1.2))
                                .px_4()
                                .py_2()
                        }))
                } else {
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .italic()
                        .child("None")
                })
        }

        div()
            .h_flex()
            .items_start()
            .gap_6()
            .w_full()
            .child(section(
                cx,
                format!("Provides ({})", self.pkg.provides.len()),
                &self.pkg.provides,
                cx.theme().blue,
            ))
            .child(section(
                cx,
                format!("Conflicts ({})", self.pkg.conflicts.len()),
                &self.pkg.conflicts,
                cx.theme().red,
            ))
    }

    fn dependencies(&self, cx: &App) -> impl IntoElement {
        div()
            .child(
                div()
                    .font_semibold()
                    .child(format!("Dependencies ({})", self.pkg.dependencies.len())),
            )
            .child(div().h_px().w_full().mt_1().mb_3().bg(cx.theme().border))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.pkg.dependencies.iter().map(|x| {
                        div()
                            .child(x.clone())
                            .bg(cx.theme().secondary)
                            .line_height(relative(1.2))
                            .px_4()
                            .py_2()
                    })),
            )
    }

    fn opt_dependencies(&self, cx: &App) -> impl IntoElement {
        div()
            .child(div().font_semibold().child(format!(
                "Optional Dependencies ({})",
                self.pkg.opt_dependencies.len()
            )))
            .child(div().h_px().w_full().mt_1().mb_3().bg(cx.theme().border))
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .children(self.pkg.opt_dependencies.iter().map(|dep| {
                        div()
                            .h_flex()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .px_4()
                            .py_3()
                            .bg(cx.theme().secondary)
                            .child(div().font_bold().child(dep.name.clone()))
                            .children(dep.reason.clone().map(|r| div().child(r)))
                    })),
            )
    }
}

impl Render for PackageDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .v_flex()
            .gap_8()
            .p_4()
            .size_full()
            .child(self.header(window, cx, entity))
            .child(self.details(cx))
            .child(self.dependencies(cx))
            .children(if self.pkg.opt_dependencies.is_empty() {
                None
            } else {
                Some(self.opt_dependencies(cx))
            })
    }
}
