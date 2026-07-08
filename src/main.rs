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

    fn info_bar(&self) -> impl IntoElement {
        fn info_item(icon: impl IntoElement, label: String) -> impl IntoElement {
            div().h_flex().gap_2().child(icon).child(label)
        }

        div()
            .h_flex()
            .gap_4()
            .child(info_item(PakajoIcon::History, self.pkg.version.clone()))
            .child(info_item(PakajoIcon::Scale, self.pkg.licenses.join(", ")))
            .children(
                self.pkg
                    .maintainer_name()
                    .map(|name| info_item(PakajoIcon::User, name)),
            )
    }

    fn header(&self) -> impl IntoElement {
        div()
            .v_flex()
            .child(div().text_2xl().child(self.format_name()))
            .child(self.info_bar())
            .children(
                self.pkg
                    .description
                    .clone()
                    .map(|desc| div().text_color(rgb(0x444444)).child(desc)),
            )
    }
}

impl Render for PackageListing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .p_4()
            .size_full()
            .child(self.header())
    }
}

struct PakajoRoot {
    alpm_handle: Alpm,
}

impl Render for PakajoRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut package: Option<&alpm::Package> = None;
        for database in self.alpm_handle.syncdbs() {
            if let Ok(pkg) = database.pkg("code") {
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
            .children(package.map_or(vec![], |pkg| {
                let package_listing = PackageListing { pkg: pkg.into() };
                vec![cx.new(|_| package_listing)]
            }))
    }
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
