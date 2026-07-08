mod package;

use alpm::{Alpm, SigLevel};
use anyhow::anyhow;
use gpui::*;
use gpui_component::*;
use rust_embed::RustEmbed;

use crate::package::Package;

#[derive(IntoElement)]
enum PakajoIcon {
    History,
}

impl IconNamed for PakajoIcon {
    fn path(self) -> SharedString {
        match self {
            PakajoIcon::History => "icons/history.svg",
        }
        .into()
    }
}

impl RenderOnce for PakajoIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::new(self)
    }
}

#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

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
        div().h_flex().gap_4().child(
            div()
                .h_flex()
                .gap_2()
                .child(PakajoIcon::History)
                .child(self.pkg.version.clone()),
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

    let app = gpui_platform::application().with_assets(Assets);

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
