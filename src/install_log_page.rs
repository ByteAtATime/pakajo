use crate::events::{DownloadResult, InstallEvent, LogLevel, PackageOp};
use crate::icon::PakajoIcon;
use crate::install::InstallProgress;
use crate::session::InstallKind;
use crate::utils::format_bytes;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use std::sync::Arc;

pub struct InstallLogPage {
    pub kind: InstallKind,
    pub name: String,
    pub logs: Vec<InstallEvent>,
    pub status: InstallProgress,
    on_back: Arc<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl InstallLogPage {
    pub fn new(
        kind: InstallKind,
        name: String,
        on_back: Arc<dyn Fn(&mut Window, &mut App) + 'static>,
    ) -> Self {
        Self {
            kind,
            name,
            logs: Vec::new(),
            status: InstallProgress::Idle,
            on_back,
        }
    }

    fn title(&self) -> String {
        match self.kind {
            InstallKind::Install => format!("Installing {}", self.name),
            InstallKind::Remove => format!("Removing {}", self.name),
        }
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (label, color): (String, Hsla) = match &self.status {
            InstallProgress::Completed => ("Completed".to_string(), cx.theme().green),
            InstallProgress::Failed(message) => (format!("Failed: {message}"), cx.theme().danger),
            InstallProgress::Cancelled => ("Cancelled".to_string(), cx.theme().muted_foreground),
            InstallProgress::Running
            | InstallProgress::Idle
            | InstallProgress::ConflictReview(_) => return None,
        };
        Some(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(cx.theme().title_bar)
                .text_color(color)
                .child(PakajoIcon::PackageCheck)
                .child(div().text_sm().child(label))
                .into_any_element(),
        )
    }

    fn render_event(ev: &InstallEvent) -> Option<Div> {
        let text = match ev {
            InstallEvent::ResolvingDependencies => ":: resolving dependencies...".to_string(),
            InstallEvent::CheckingConflicts => ":: checking for conflicts...".to_string(),
            InstallEvent::CheckingFileConflicts => ":: checking for file conflicts...".to_string(),
            InstallEvent::CheckingIntegrity => ":: checking package integrity...".to_string(),
            InstallEvent::CheckingDiskSpace => ":: checking available disk space...".to_string(),
            InstallEvent::LoadingPackages => ":: loading package files...".to_string(),
            InstallEvent::KeyringStart => ":: checking keyring...".to_string(),
            InstallEvent::RetrievingPackages { num, total_bytes } => {
                format!(":: retrieving {num} packages ({})", format_bytes(*total_bytes))
            }
            InstallEvent::PackageOperation {
                operation,
                package,
                new_version,
                old_version,
            } => format_package_operation(*operation, package, new_version, old_version),
            InstallEvent::DownloadInit {
                filename,
                optional: true,
            } => format!("  {filename} (optional)"),
            InstallEvent::DownloadInit { .. } => return None,
            InstallEvent::DownloadRetry { filename, resume } => {
                let kind = if *resume { "resumable" } else { "full" };
                format!("  {filename}: retrying ({kind})")
            }
            InstallEvent::DownloadCompleted {
                filename,
                total,
                result,
            } => {
                let status = match result {
                    DownloadResult::Success => "done",
                    DownloadResult::UpToDate => "up to date",
                    DownloadResult::Failed => "failed",
                };
                format!("  {filename}: {} [{status}]", format_bytes(*total))
            }
            InstallEvent::Progress { .. } | InstallEvent::DownloadProgress { .. } => return None,
            InstallEvent::HookRun {
                position,
                total,
                name,
                desc,
            } => {
                let label = desc.as_deref().unwrap_or(name);
                format!(":: running hook ({position}/{total}): {label}")
            }
            InstallEvent::ScriptletInfo { line } => line.clone(),
            InstallEvent::Log { level, message } => match level {
                LogLevel::Error => format!("error: {message}"),
                LogLevel::Warning => format!("warning: {message}"),
                LogLevel::Debug => return None,
            },
            InstallEvent::TransactionDone
            | InstallEvent::TransactionSummary(_)
            | InstallEvent::AurDepResolved { .. }
            | InstallEvent::ResolutionComplete { .. }
            | InstallEvent::LayerBoundary { .. }
            | InstallEvent::SysupgradeAurCandidates { .. } => return None,
            InstallEvent::ResolvingAurDependencies { target } => {
                format!(":: resolving dependencies for {target}...")
            }
            InstallEvent::CloningRepo { package } => {
                format!(":: retrieving build files for {package}...")
            }
            InstallEvent::BuildStarted { package } => format!(":: building {package}..."),
            InstallEvent::BuildOutput { line, .. } => format!("  {line}"),
            InstallEvent::BuildCompleted {
                package, version, ..
            } => match version {
                Some(version) => format!(":: built {package} {version}"),
                None => format!(":: built {package}"),
            },
        };
        Some(div().child(text))
    }
}

fn format_package_operation(
    operation: PackageOp,
    package: &str,
    new_version: &Option<String>,
    old_version: &Option<String>,
) -> String {
    let new = new_version.as_deref().unwrap_or("?");
    let old = old_version.as_deref().unwrap_or("?");
    match operation {
        PackageOp::Install => format!("installing {package} ({new})"),
        PackageOp::Upgrade => format!("upgrading {package} ({old} -> {new})"),
        PackageOp::Reinstall => format!("reinstalling {package} ({new})"),
        PackageOp::Downgrade => format!("downgrading {package} ({old} -> {new})"),
        PackageOp::Remove => format!("removing {package} ({old})"),
    }
}

impl Render for InstallLogPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on_back = self.on_back.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_lg().font_semibold().child(self.title()))
                    .child(
                        Button::new("install-back")
                            .label("Back")
                            .ghost()
                            .rounded_none()
                            .on_click(move |_, window, cx| on_back(window, cx)),
                    ),
            )
            .children(self.render_banner(cx))
            .child(
                div()
                    .id("install-log-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .v_flex()
                    .gap_1()
                    .p_3()
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
                    .text_size(rems(0.8))
                    .children(self.logs.iter().filter_map(InstallLogPage::render_event)),
            )
    }
}
