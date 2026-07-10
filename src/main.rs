mod icon;
mod package;

use alpm::{Alpm, SigLevel};
use gpui::*;
use gpui_component::*;

use crate::{icon::PakajoIcon, package::Package};

struct PackageListing {
    pkg: Package,
}

impl PackageListing {
    fn format_name(&self) -> String {
        if let Some(repo) = self.pkg.repo.clone() {
            format!("{}/{}", repo, self.pkg.name)
        } else {
            self.pkg.name.clone()
        }
    }

    fn info_bar(&self, cx: &App) -> impl IntoElement {
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
            .child(format_size(self.pkg.installed_size))
            .child(div().text_color(cx.theme().muted_foreground).child("/"))
            .child(format_size(self.pkg.download_size));

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

    fn header(&self, window: &Window, cx: &App) -> impl IntoElement {
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

        fn title(cx: &App, name: String, version: String, window: &Window) -> impl IntoElement {
            let name_size = 2.0;
            let version_size = 1.5;
            let pad = baseline_from_top(window, &name, name_size)
                - baseline_from_top(window, &version, version_size);

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
        }

        div()
            .v_flex()
            .child(title(
                cx,
                self.format_name(),
                self.pkg.version.clone(),
                window,
            ))
            .children(
                self.pkg
                    .description
                    .clone()
                    .map(|desc| div().text_color(cx.theme().muted_foreground).child(desc)),
            )
            .child(self.info_bar(cx))
    }

    fn details(&self, cx: &App) -> impl IntoElement {
        fn section(cx: &App, title: String, items: &[String], color: Hsla) -> impl IntoElement {
            div()
                .v_flex()
                .w_full()
                .child(div().font_semibold().child(title))
                .child(div().h_px().w_full().mt_1().mb_3().bg(cx.theme().border))
                .child(if items.len() > 0 {
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
        div()
            .v_flex()
            .gap_8()
            .p_4()
            .size_full()
            .child(self.header(window, cx))
            .child(self.details(cx))
            .child(self.dependencies(cx))
            .child(self.opt_dependencies(cx))
    }
}

struct PakajoRoot {
    alpm_handle: Alpm,
}

impl Render for PakajoRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut package: Option<&alpm::Package> = None;
        for database in self.alpm_handle.syncdbs() {
            if let Ok(pkg) = database.pkg("mariadb") {
                package = Some(pkg);
                break;
            }
        }

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .items_center()
            .justify_center()
            .font_family("Inter")
            .children(package.map_or(vec![], |pkg| {
                let package_listing = PackageListing { pkg: pkg.into() };
                vec![cx.new(|_| package_listing)]
            }))
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

fn main() {
    let config = pacmanconf::Config::new().expect("Couldn't parse pacman.conf");

    let handle = Alpm::new(config.root_dir, config.db_path).expect("Couldn't initialize alpm");
    for repo in &config.repos {
        handle
            .register_syncdb(repo.name.clone(), parse_siglevel(&repo.sig_level))
            .unwrap();
    }

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
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}
