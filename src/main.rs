mod cli;
mod events;
mod icon;
mod install;
mod package;
mod package_listing;
mod pacman;
mod root;
mod utils;

use anyhow::Context as _;
use gpui::*;
use gpui_component::*;
use crate::{pacman::init_alpm, root::{PakajoRoot, InstallProgress}};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("install") {
        cli::install_subcommand(args);
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
                    target_package: "sl".to_string(),
                    package_listing: None,
                    install_progress: InstallProgress::Idle,
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });

    Ok(())
}
