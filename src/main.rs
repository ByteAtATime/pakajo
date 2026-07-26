mod answerer;
mod aur;
mod build;
mod clean;
mod cli;
mod color;
mod devel;
mod dry_run;
mod events;
mod git;
mod icon;
mod install;
mod install_page;
mod install_review_dialog;
mod local_index;
mod lookup;
mod package;
mod package_detail;
mod pacman;
mod pacman_watch;
mod pkgbuild;
mod question;
mod remove;
mod resolve;
mod root;
mod search;
mod search_view;
mod session;
mod srcinfo_io;
mod stats;
mod stub_pkg;
mod upgrade;
mod utils;

use crate::{pacman::init_alpm, root::PakajoRoot};
use anyhow::Context as _;
use gpui::*;
use gpui_component::*;

fn main() -> anyhow::Result<()> {
    let cli = cli::parse();
    match cli.command {
        Some(cli::Command::Install(a)) => cli::install_subcommand(a),
        Some(cli::Command::Remove(a)) => cli::remove_subcommand(a),
        Some(cli::Command::Upgrade(a)) => cli::upgrade_subcommand(a),
        Some(cli::Command::Search(a)) => cli::search_subcommand(a),
        Some(cli::Command::Clean(a)) => cli::clean_subcommand(a),
        Some(cli::Command::AurSync) => cli::aur_sync_subcommand(),
        Some(cli::Command::Gendb) => cli::gendb_subcommand(),
        None => {}
    }

    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let handle = init_alpm(&config)?;
    let aur_client = crate::aur::AurClient::new();

    let app = gpui_platform::application().with_assets(icon::CombinedAssets);

    app.run(move |cx| {
        gpui_component::init(cx);
        root::init(cx);

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
                let view = cx.new(|cx| PakajoRoot::new(&mut *window, cx, handle, aur_client));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });

    Ok(())
}
