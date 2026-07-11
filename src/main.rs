mod icon;
mod install;
mod package;

use std::rc::Rc;

use alpm::{Alpm, SigLevel};
use anyhow::Context as _;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    tooltip::Tooltip,
    *,
};

use crate::{
    icon::PakajoIcon,
    package::{Package, is_installed},
};

#[derive(Clone, Copy, PartialEq)]
enum SizeTooltipTarget {
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

struct PackageListing {
    pkg: Package,
    installed: bool,
    active_tooltip: Option<SizeTooltipTarget>,
}

impl PackageListing {
    fn format_name(&self) -> String {
        if let Some(repo) = self.pkg.repo.clone() {
            format!("{}/{}", repo, self.pkg.name)
        } else {
            self.pkg.name.clone()
        }
    }

    fn sized_value(
        &self,
        entity: &Entity<PackageListing>,
        target: SizeTooltipTarget,
        value: i64,
    ) -> Stateful<Div> {
        let active_tooltip = self.active_tooltip;
        let tooltip_text = format!("{}: {}", target.label(), format_size(value));

        div()
            .child(format_size(value))
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
    }

    fn info_bar(&self, cx: &App, entity: Entity<PackageListing>) -> impl IntoElement {
        fn info_item(cx: &App, icon: PakajoIcon, label: String) -> impl IntoElement {
            div()
                .h_flex()
                .gap_2()
                .child(Icon::new(icon).text_color(cx.theme().muted_foreground))
                .child(label)
        }

        let sizes = div()
            .h_flex()
            .gap_2()
            .child(Icon::new(PakajoIcon::HardDrive).text_color(cx.theme().muted_foreground))
            .child(self.sized_value(&entity, SizeTooltipTarget::Download, self.pkg.download_size))
            .child(div().text_color(cx.theme().muted_foreground).child("/"))
            .child(self.sized_value(
                &entity,
                SizeTooltipTarget::Installed,
                self.pkg.installed_size,
            ));

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
            .child(sizes)
    }

    fn header(
        &self,
        window: &Window,
        cx: &App,
        entity: Entity<PackageListing>,
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
            window: &Window,
        ) -> impl IntoElement {
            let name_size = 2.0;
            let version_size = 1.5;
            let pad = baseline_from_top(window, &name, name_size)
                - baseline_from_top(window, &version, version_size);

            let install_button = Button::new("install-button")
                .label(if installed { "Installed" } else { "Install" })
                .disabled(installed)
                .rounded_none()
                .large()
                .on_click(|_, _, _| {});
            let install_button = if installed {
                install_button
            } else {
                install_button.primary()
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
                .child(install_button.ml_auto())
        }

        div()
            .v_flex()
            .child(title(
                cx,
                self.format_name(),
                self.pkg.version.clone(),
                self.installed,
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

impl Render for PackageListing {
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
            .child(self.opt_dependencies(cx))
    }
}

struct PakajoRoot {
    alpm_handle: Alpm,
    target_package: String,
    package_listing: Option<Entity<PackageListing>>,
}

impl Render for PakajoRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.package_listing.is_none() {
            self.package_listing = find_pkg(&self.alpm_handle, &self.target_package).map(|pkg| {
                let package: Package = pkg.into();
                let installed = is_installed(&self.alpm_handle, &package.name);
                cx.new(|_| PackageListing {
                    pkg: package,
                    installed,
                    active_tooltip: None,
                })
            });
        }

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .items_center()
            .justify_center()
            .font_family("Inter")
            .children(self.package_listing.clone())
    }
}

fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    if bytes < 1024 {
        return format!("{} {}", bytes, UNITS[0]);
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    format!("{:.1} {}", value, UNITS[unit])
}

fn parse_siglevel(sig_strings: &[String]) -> SigLevel {
    if sig_strings.is_empty() {
        return SigLevel::USE_DEFAULT;
    }

    let mut level = SigLevel::empty();

    for s in sig_strings {
        let flag = match s.as_str() {
            "Never" => SigLevel::NONE,
            "Optional" => SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL,
            "Required" => SigLevel::PACKAGE,
            "TrustedOnly" => SigLevel::empty(),
            "TrustAll" => SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK,

            "DatabaseOptional" => SigLevel::DATABASE | SigLevel::DATABASE_OPTIONAL,
            "DatabaseRequired" => SigLevel::DATABASE,
            "DatabaseTrustedOnly" => SigLevel::empty(),
            "DatabaseTrustAll" => SigLevel::DATABASE_MARGINAL_OK | SigLevel::DATABASE_UNKNOWN_OK,

            "PackageOptional" => SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL,
            "PackageRequired" => SigLevel::PACKAGE,
            "PackageTrustedOnly" => SigLevel::empty(),
            "PackageTrustAll" => SigLevel::PACKAGE_MARGINAL_OK | SigLevel::PACKAGE_UNKNOWN_OK,

            other => SigLevel::from_name(other).unwrap_or_else(SigLevel::empty),
        };

        level = level.union(flag);
    }

    if level.is_empty() {
        SigLevel::USE_DEFAULT
    } else {
        level
    }
}

fn init_alpm(config: &pacmanconf::Config) -> anyhow::Result<Alpm> {
    let mut handle = Alpm::new(config.root_dir.clone(), config.db_path.clone())?;
    handle.set_architectures(config.architecture.iter())?;
    for repo in &config.repos {
        let db = handle.register_syncdb_mut(repo.name.clone(), parse_siglevel(&repo.sig_level))?;
        db.set_servers(repo.servers.iter())?;
    }
    Ok(handle)
}

fn find_pkg<'a>(handle: &'a Alpm, name: &str) -> Option<&'a alpm::Package> {
    handle.syncdbs().iter().find_map(|db| db.pkg(name).ok())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some("install") = args.next().as_deref() {
        let name = match args.next() {
            Some(name) => name,
            None => {
                eprintln!("usage: pakajo install <package>");
                std::process::exit(2);
            }
        };

        if unsafe { libc::geteuid() } != 0 {
            let exe = std::env::current_exe().context("failed to determine executable path")?;
            let status = std::process::Command::new("sudo")
                .arg(&exe)
                .arg("install")
                .arg(&name)
                .status()
                .context("failed to run sudo")?;
            std::process::exit(status.code().unwrap_or(1));
        }

        if let Err(e) = install::run_install(&name) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let handle = init_alpm(&config)?;

    let app = gpui_platform::application().with_assets(icon::Assets);

    app.run(move |cx| {
        gpui_component::init(cx);

        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(include_str!("tokyonight.json"))
            .expect("Failed to load theme");

        if let Some(theme_config) = ThemeRegistry::global(cx)
            .themes()
            .get(&SharedString::new("Tokyo Night"))
            .cloned()
        {
            Theme::global_mut(cx).apply_config(&theme_config);
        }

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| PakajoRoot {
                    alpm_handle: handle,
                    target_package: "ripgrep".to_string(),
                    package_listing: None,
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });

    Ok(())
}
