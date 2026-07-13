use std::{io::{self, BufRead}, process::{Command, ExitStatus, Stdio}};
use alpm::Alpm;
use futures::StreamExt as _;
use gpui::*;
use gpui_component::{ActiveTheme as _, StyledExt as _};
use crate::{aur::AurClient, lookup::lookup, pacman::init_alpm, package::is_installed, package_listing::PackageListing};

#[derive(Clone, Debug, PartialEq)]
pub enum InstallProgress {
    Idle,
    Running,
    Failed(String),
}

#[derive(Clone, Debug)]
enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

enum StreamItem {
    Event(crate::events::InstallEvent),
    Done(ChildOutcome),
}

pub struct PakajoRoot {
    pub alpm_handle: Alpm,
    pub aur_client: AurClient,
    pub target_package: String,
    pub package_listing: Option<Entity<PackageListing>>,
    pub lookup_attempted: bool,
    pub install_progress: InstallProgress,
}

impl PakajoRoot {
    fn set_progress(&mut self, progress: InstallProgress, cx: &mut Context<Self>) {
        if let Some(listing) = &self.package_listing {
            listing.update(cx, |listing, cx| {
                listing.install_progress = progress.clone();
                cx.notify();
            });
        }
        self.install_progress = progress;
        cx.notify();
    }

    pub fn start_install(&mut self, cx: &mut Context<Self>) {
        if matches!(self.install_progress, InstallProgress::Running) {
            return;
        }
        self.set_progress(InstallProgress::Running, cx);

        let name = self.target_package.clone();
        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.set_progress(
                    InstallProgress::Failed(format!(
                        "failed to determine executable path: {error}"
                    )),
                    cx,
                );
                return;
            }
        };

        let (mut tx, mut rx) = futures::channel::mpsc::channel::<StreamItem>(256);

        std::thread::spawn(move || {
            let mut send_event = |mut item: StreamItem| loop {
                match tx.try_send(item) {
                    Ok(()) => return,
                    Err(err) => {
                        if err.is_disconnected() {
                            return;
                        }
                        item = err.into_inner();
                        std::thread::yield_now();
                    }
                }
            };

            let outcome = match Command::new(&exe)
                .arg("install")
                .arg("--json")
                .arg(&name)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
            {
                Err(error) if error.kind() == io::ErrorKind::NotFound => ChildOutcome::NotFound,
                Err(error) => ChildOutcome::Failed(error.to_string()),
                Ok(mut child) => {
                    let stdout = child.stdout.take().expect("piped");
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(l) => {
                                if let Ok(ev) =
                                    serde_json::from_str::<crate::events::InstallEvent>(&l)
                                {
                                    send_event(StreamItem::Event(ev));
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    map_outcome(child.wait())
                }
            };
            send_event(StreamItem::Done(outcome));
        });

        cx.spawn(async move |this, cx| {
            while let Some(item) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_stream_item(item, cx));
            }
        })
        .detach();
    }

    fn handle_stream_item(&mut self, item: StreamItem, cx: &mut Context<Self>) {
        match item {
            StreamItem::Event(ev) => {
                eprintln!("{ev:?}");
            }
            StreamItem::Done(ChildOutcome::Success) => {
                self.refresh_after_install(cx);
                self.set_progress(InstallProgress::Idle, cx);
            }
            StreamItem::Done(ChildOutcome::Dismissed) => {
                self.set_progress(InstallProgress::Idle, cx);
            }
            StreamItem::Done(ChildOutcome::NotFound) => {
                self.set_progress(
                    InstallProgress::Failed("install child not found".into()),
                    cx,
                );
            }
            StreamItem::Done(ChildOutcome::Failed(message)) => {
                self.set_progress(InstallProgress::Failed(message), cx);
            }
        }
    }

    fn refresh_after_install(&mut self, cx: &mut Context<Self>) {
        if let Ok(config) = pacmanconf::Config::new()
            && let Ok(handle) = init_alpm(&config)
        {
            self.alpm_handle = handle;
        }

        let installed = if let Some(listing) = &self.package_listing {
            let pkg_name = listing.read(cx).pkg.name.clone();
            is_installed(&self.alpm_handle, &pkg_name)
        } else {
            false
        };

        if let Some(listing) = &self.package_listing {
            listing.update(cx, |listing, cx| {
                listing.installed = installed;
                cx.notify();
            });
        }
    }
}

impl Render for PakajoRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.lookup_attempted {
            self.lookup_attempted = true;
            let weak_root = cx.weak_entity();
            self.package_listing = lookup(&self.alpm_handle, &self.aur_client, &self.target_package).map(|package| {
                let installed = is_installed(&self.alpm_handle, &package.name);
                cx.new(|_| PackageListing {
                    pkg: package,
                    installed,
                    active_tooltip: None,
                    root: weak_root,
                    install_progress: self.install_progress.clone(),
                })
            });
        }

        let not_found = (self.lookup_attempted && self.package_listing.is_none()).then(|| {
            div()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Package '{}' not found", self.target_package))
        });

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .items_center()
            .justify_center()
            .font_family("Inter")
            .children(self.package_listing.clone())
            .children(not_found)
    }
}

fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}
