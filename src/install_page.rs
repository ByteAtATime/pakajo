use crate::color;
use crate::events::{
    DownloadResult, InstallEvent, LogLevel, PackageOp, ProgressPhase, TransactionSummary,
};
use crate::install::InstallProgress;
use crate::package::PackageSource;
use crate::session::InstallKind;
use crate::utils::format_bytes;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    v_flex,
};
use std::collections::HashMap;
use std::sync::Arc;

enum PageMode {
    Repo(RepoState),
    Aur(AurState),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RepoStage {
    Resolve,
    Validate,
    Download,
    Install,
    Finalize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AurStage {
    Resolve,
    Build,
    Validate,
    Install,
    Finalize,
}

struct RepoState {
    manifest: Option<TransactionSummary>,
    stage: RepoStage,
    scroll: ScrollHandle,
    download_total: usize,
    download_done: usize,
    download_bytes_total: i64,
    download_bytes_done: i64,
    download_files: HashMap<String, i64>,
}

struct AurState {
    manifest: Option<TransactionSummary>,
    stage: AurStage,
    building: Option<String>,
    cloning: Option<String>,
    scroll: ScrollHandle,
}

pub struct InstallPage {
    pub kind: InstallKind,
    pub name: String,
    pub logs: Vec<InstallEvent>,
    pub status: InstallProgress,
    pub overall: f32,
    pub indeterminate: bool,
    pub status_text: Option<String>,
    mode: PageMode,
    on_back: Arc<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl InstallPage {
    pub fn new(
        kind: InstallKind,
        source: PackageSource,
        name: String,
        on_back: Arc<dyn Fn(&mut Window, &mut App) + 'static>,
    ) -> Self {
        let mode = match source {
            PackageSource::Repo => PageMode::Repo(RepoState {
                manifest: None,
                stage: RepoStage::Resolve,
                scroll: ScrollHandle::new(),
                download_total: 0,
                download_done: 0,
                download_bytes_total: 0,
                download_bytes_done: 0,
                download_files: HashMap::new(),
            }),
            PackageSource::Aur => PageMode::Aur(AurState {
                manifest: None,
                stage: AurStage::Resolve,
                building: None,
                cloning: None,
                scroll: ScrollHandle::new(),
            }),
            PackageSource::Group => panic!("group install not implemented until Phase 4"),
        };
        Self {
            kind,
            name,
            logs: Vec::new(),
            status: InstallProgress::Idle,
            overall: 0.0,
            indeterminate: true,
            status_text: None,
            on_back,
            mode,
        }
    }

    pub fn handle_event(&mut self, ev: InstallEvent, cx: &mut Context<Self>) {
        self.status_text = None;
        if let PageMode::Repo(state) = &mut self.mode
            && let Some(new_stage) = event_stage(&ev)
            && new_stage != state.stage
        {
            let ordered = ordered_stages(self.kind);
            let current_idx = ordered.iter().position(|s| *s == state.stage);
            let new_idx = ordered.iter().position(|s| *s == new_stage);
            if let (Some(current_idx), Some(new_idx)) = (current_idx, new_idx)
                && new_idx > current_idx
            {
                state.stage = new_stage;
            }
        } else if let PageMode::Aur(state) = &mut self.mode {
            if let Some(new_stage) = aur_event_stage(&ev)
                && new_stage != state.stage
            {
                let ordered = ordered_aur_stages();
                let current_idx = ordered.iter().position(|s| *s == state.stage);
                let new_idx = ordered.iter().position(|s| *s == new_stage);
                if let (Some(current_idx), Some(new_idx)) = (current_idx, new_idx)
                    && new_idx > current_idx
                {
                    state.stage = new_stage;
                }
            }
            match &ev {
                InstallEvent::CloningRepo { package } => state.cloning = Some(package.clone()),
                InstallEvent::BuildStarted { package } => state.building = Some(package.clone()),
                InstallEvent::BuildCompleted { .. } => state.building = None,
                _ => {}
            }
        }
        if let PageMode::Repo(state) = &mut self.mode {
            apply_repo_counters(state, &ev);
        }
        if let PageMode::Repo(state) = &self.mode
            && state.stage == RepoStage::Download
            && state.download_bytes_total > 0
        {
            self.indeterminate = false;
            self.overall = ((state.download_bytes_done as f32 / state.download_bytes_total as f32)
                * 100.0)
                .min(100.0);
            self.status_text = Some(format!(
                "{} / {}",
                format_bytes(state.download_bytes_done),
                format_bytes(state.download_bytes_total),
            ));
        }
        match &ev {
            InstallEvent::BuildStarted { .. } => {
                self.indeterminate = true;
            }
            _ => {}
        }
        match ev {
            InstallEvent::Progress {
                phase,
                percent,
                current,
                total,
                ..
            } => {
                let is_op = matches!(
                    phase,
                    ProgressPhase::Add
                        | ProgressPhase::Upgrade
                        | ProgressPhase::Downgrade
                        | ProgressPhase::Reinstall
                        | ProgressPhase::Remove
                );
                if is_op && total > 0 {
                    self.indeterminate = false;
                    self.overall = ((current.saturating_sub(1) as f32 + percent as f32 / 100.0)
                        / total as f32
                        * 100.0)
                        .min(100.0);
                } else {
                    self.indeterminate = true;
                }
            }
            InstallEvent::DownloadProgress { .. } => {}
            InstallEvent::TransactionSummary(summary) => match &mut self.mode {
                PageMode::Repo(state) => state.manifest = Some(summary),
                PageMode::Aur(state) => state.manifest = Some(summary),
            },
            other => {
                self.logs.push(other);
            }
        }
        cx.notify();
    }

    fn title(&self) -> String {
        match self.kind {
            InstallKind::Install => format!("Installing {}", self.name),
            InstallKind::Remove => format!("Removing {}", self.name),
            InstallKind::Upgrade => format!("Upgrading {}", self.name),
        }
    }

    fn render_event(ev: &InstallEvent) -> Option<Div> {
        let text = match ev {
            InstallEvent::ResolvingDependencies => ":: resolving dependencies...".to_string(),
            InstallEvent::CheckingConflicts => ":: checking for conflicts...".to_string(),
            InstallEvent::CheckingDependencies => ":: checking dependencies...".to_string(),
            InstallEvent::CheckingFileConflicts => ":: checking for file conflicts...".to_string(),
            InstallEvent::CheckingIntegrity => ":: checking package integrity...".to_string(),
            InstallEvent::CheckingDiskSpace => ":: checking available disk space...".to_string(),
            InstallEvent::LoadingPackages => "loading packages...".to_string(),
            InstallEvent::KeyringStart => ":: checking keyring...".to_string(),
            InstallEvent::RetrievingPackages { num, total_bytes } => {
                format!(
                    ":: retrieving {num} packages ({})",
                    format_bytes(*total_bytes)
                )
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
            | InstallEvent::SysupgradeAurCandidates { .. }
            | InstallEvent::PkgbuildReviewStarted { .. }
            | InstallEvent::PkgbuildReviewAccepted { .. }
            | InstallEvent::PkgbuildAllUpToDate { .. }
            | InstallEvent::ProcessingChanges => return None,
            InstallEvent::ResolvingAurDependencies { target } => {
                format!(":: resolving dependencies for {target}...")
            }
            InstallEvent::CloningRepo { package } => {
                format!(":: retrieving build files for {package}...")
            }
            InstallEvent::BuildStarted { package } => format!(":: building {package}..."),
            InstallEvent::BuildOutput { line, .. } => {
                format!("  {}", color::ansi_strip(line))
            }
            InstallEvent::BuildCompleted {
                package, version, ..
            } => match version {
                Some(version) => format!(":: built {package} {version}"),
                None => format!(":: built {package}"),
            },
        };
        Some(div().child(text))
    }

    fn render_terminal(&self, cx: &mut Context<Self>, scroll: &ScrollHandle) -> Stateful<Div> {
        let offset = scroll.offset();
        let max = scroll.max_offset();
        let at_bottom = offset.y <= -max.y + px(6.);
        if at_bottom {
            scroll.scroll_to_bottom();
        }
        div()
            .id("install-log-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(scroll)
            .v_flex()
            .gap_1()
            .p_3()
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .text_size(rems(0.8))
            .children(self.logs.iter().filter_map(InstallPage::render_event))
    }

    fn render_aur(&self, cx: &mut Context<Self>) -> Div {
        let on_back = self.on_back.clone();
        let PageMode::Aur(state) = &self.mode else {
            return div();
        };
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
            .child(self.render_aur_top(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap_4()
                    .child(self.render_aur_stages(cx))
                    .child(self.render_terminal(cx, &state.scroll)),
            )
    }

    fn render_aur_top(&self, cx: &mut Context<Self>) -> Div {
        let PageMode::Aur(state) = &self.mode else {
            return div();
        };
        if let Some(summary) = state.manifest.as_ref().filter(|s| !s.packages.is_empty()) {
            return Self::render_manifest_card(self.kind, summary, cx);
        }
        match &self.status {
            InstallProgress::Failed(message) => v_flex()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .p_4()
                .text_color(cx.theme().danger)
                .child(format!("Transaction failed: {message}")),
            _ => {
                let label = match state.stage {
                    AurStage::Resolve => "Resolving AUR dependencies…".to_string(),
                    AurStage::Build => state
                        .building
                        .as_deref()
                        .map(|p| format!("Building {p}…"))
                        .unwrap_or_else(|| "Building…".to_string()),
                    _ => "Preparing transaction…".to_string(),
                };
                v_flex()
                    .gap_2()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    )
                    .child(Progress::new("aur-top-prep").loading(true))
            }
        }
    }

    fn render_stages(&self, cx: &mut Context<Self>) -> Div {
        let PageMode::Repo(state) = &self.mode else {
            return div();
        };
        let ordered = ordered_stages(self.kind);
        let current_idx = ordered.iter().position(|s| *s == state.stage).unwrap_or(0);
        let mut card = v_flex()
            .gap_2()
            .w(rems(22.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .p_4();
        for (i, stage) in ordered.iter().enumerate() {
            let cell = cell_state(&self.status, current_idx, i);
            let (glyph, glyph_color, label_color) = match cell {
                CellState::Done => ("✓", cx.theme().green, cx.theme().muted_foreground),
                CellState::Active => ("●", cx.theme().blue, cx.theme().foreground),
                CellState::Failed => ("✗", cx.theme().danger, cx.theme().danger),
                CellState::Pending => (
                    "○",
                    cx.theme().muted_foreground,
                    cx.theme().muted_foreground,
                ),
            };
            let label = match stage {
                RepoStage::Resolve => "Resolve",
                RepoStage::Validate => "Validate",
                RepoStage::Download => "Download",
                RepoStage::Install => match self.kind {
                    InstallKind::Install => "Install",
                    InstallKind::Remove => "Remove",
                    InstallKind::Upgrade => "Upgrade",
                },
                RepoStage::Finalize => "Finalize",
            };
            let count = if matches!(cell, CellState::Active)
                && matches!(stage, RepoStage::Download)
                && state.download_total > 0
            {
                Some(format!("{}/{}", state.download_done, state.download_total))
            } else {
                None
            };
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(glyph_color).child(glyph))
                    .child(div().text_color(label_color).child(label))
                    .children(count.map(|c| {
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(c)
                    })),
            );
        }
        if let InstallProgress::Failed(message) = &self.status {
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(cx.theme().danger).child("✗"))
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child(format!("Failed: {message}")),
                    ),
            );
        } else if matches!(self.status, InstallProgress::Completed) {
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(cx.theme().green).child("✓"))
                    .child(div().text_color(cx.theme().green).child("Done")),
            );
        }
        card
    }

    fn render_aur_stages(&self, cx: &mut Context<Self>) -> Div {
        let PageMode::Aur(state) = &self.mode else {
            return div();
        };
        let ordered = ordered_aur_stages();
        let current_idx = ordered.iter().position(|s| *s == state.stage).unwrap_or(0);
        let mut card = v_flex()
            .gap_2()
            .w(rems(22.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .p_4();
        for (i, stage) in ordered.iter().enumerate() {
            let (glyph, glyph_color, label_color) = match cell_state(&self.status, current_idx, i) {
                CellState::Done => ("✓", cx.theme().green, cx.theme().muted_foreground),
                CellState::Active => ("●", cx.theme().blue, cx.theme().foreground),
                CellState::Failed => ("✗", cx.theme().danger, cx.theme().danger),
                CellState::Pending => (
                    "○",
                    cx.theme().muted_foreground,
                    cx.theme().muted_foreground,
                ),
            };
            let label = match stage {
                AurStage::Resolve => "Resolve",
                AurStage::Build => "Build",
                AurStage::Validate => "Validate",
                AurStage::Install => "Install",
                AurStage::Finalize => "Finalize",
            };
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(glyph_color).child(glyph))
                    .child(div().text_color(label_color).child(label)),
            );
        }
        if let InstallProgress::Failed(message) = &self.status {
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(cx.theme().danger).child("✗"))
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child(format!("Failed: {message}")),
                    ),
            );
        } else if matches!(self.status, InstallProgress::Completed) {
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(cx.theme().green).child("✓"))
                    .child(div().text_color(cx.theme().green).child("Done")),
            );
        }
        card
    }

    fn render_repo(&self, cx: &mut Context<Self>) -> Div {
        let on_back = self.on_back.clone();
        let PageMode::Repo(state) = &self.mode else {
            return div();
        };
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
            .child(self.render_summary(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap_4()
                    .child(self.render_stages(cx))
                    .child(self.render_terminal(cx, &state.scroll)),
            )
    }

    pub(crate) fn render_manifest_card(
        kind: InstallKind,
        summary: &TransactionSummary,
        cx: &App,
    ) -> Div {
        let mut installs = 0u32;
        let mut upgrades = 0u32;
        let mut removes = 0u32;
        for pkg in &summary.packages {
            if pkg.is_removal {
                removes += 1;
            } else if pkg.old_version.is_some() {
                upgrades += 1;
            } else {
                installs += 1;
            }
        }

        let mut segments: Vec<String> = Vec::new();
        if installs > 0 {
            segments.push(format!("Install {installs}"));
        }
        if upgrades > 0 {
            segments.push(format!("Upgrade {upgrades}"));
        }
        if removes > 0 {
            segments.push(format!("Remove {removes}"));
        }
        let counts_left = segments.join(" · ");

        let download_right = match kind {
            InstallKind::Install | InstallKind::Upgrade if summary.total_download_size > 0 => {
                Some(format!("↓ {}", format_bytes(summary.total_download_size)))
            }
            InstallKind::Install | InstallKind::Upgrade => None,
            InstallKind::Remove => None,
        };

        let net_text = match kind {
            InstallKind::Install | InstallKind::Upgrade => {
                let net = summary.total_installed_size - summary.total_removed_size;
                if net != 0 {
                    let sign = if net > 0 { "+" } else { "-" };
                    Some(format!("Net {sign}{}", format_bytes(net.abs())))
                } else {
                    None
                }
            }
            InstallKind::Remove if summary.total_removed_size > 0 => Some(format!(
                "Frees {}",
                format_bytes(summary.total_removed_size)
            )),
            InstallKind::Remove => None,
        };

        let mut card = v_flex()
            .gap_3()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .p_4()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().child(counts_left))
                    .children(download_right.map(|d| div().child(d))),
            );

        if let Some(net) = net_text {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(net),
            );
        }

        card = card.child(div().h_px().w_full().bg(cx.theme().border));

        let total = summary.packages.len();
        for pkg in summary.packages.iter().take(6) {
            let (op_label, op_color, version_text) = if pkg.is_removal {
                (
                    "Remove",
                    cx.theme().danger,
                    pkg.old_version.clone().unwrap_or_default(),
                )
            } else if pkg.old_version.is_some() {
                (
                    "Upgrade",
                    cx.theme().blue,
                    format!(
                        "{} → {}",
                        pkg.old_version.as_deref().unwrap_or("?"),
                        pkg.new_version,
                    ),
                )
            } else {
                ("Install", cx.theme().green, pkg.new_version.clone())
            };
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(pkg.name.clone()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(version_text),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(op_color)
                            .bg(op_color.opacity(0.1))
                            .px_2()
                            .child(op_label),
                    ),
            );
        }

        let extra = total.saturating_sub(6);
        if extra > 0 {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("+{extra} more")),
            );
        }

        card
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> Div {
        let PageMode::Repo(state) = &self.mode else {
            return div();
        };
        match &state.manifest {
            Some(summary) if !summary.packages.is_empty() => {
                Self::render_manifest_card(self.kind, summary, cx)
            }
            Some(_) => v_flex()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .p_4()
                .text_color(cx.theme().muted_foreground)
                .child("Nothing to do"),
            None => match &self.status {
                InstallProgress::Failed(message) => v_flex()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_4()
                    .text_color(cx.theme().danger)
                    .child(format!("Transaction failed: {message}")),
                _ => v_flex()
                    .gap_2()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Preparing transaction…"),
                    )
                    .child(Progress::new("install-summary-prep").loading(true)),
            },
        }
    }
}

fn ordered_stages(kind: InstallKind) -> Vec<RepoStage> {
    use RepoStage::*;
    match kind {
        InstallKind::Install | InstallKind::Upgrade => {
            vec![Resolve, Validate, Download, Install, Finalize]
        }
        InstallKind::Remove => vec![Resolve, Validate, Install, Finalize],
    }
}

enum CellState {
    Done,
    Active,
    Failed,
    Pending,
}

fn cell_state(status: &InstallProgress, current_idx: usize, idx: usize) -> CellState {
    match status {
        InstallProgress::Completed => CellState::Done,
        InstallProgress::Failed(_) => {
            if idx < current_idx {
                CellState::Done
            } else if idx == current_idx {
                CellState::Failed
            } else {
                CellState::Pending
            }
        }
        _ => {
            if idx < current_idx {
                CellState::Done
            } else if idx == current_idx {
                CellState::Active
            } else {
                CellState::Pending
            }
        }
    }
}

fn apply_repo_counters(state: &mut RepoState, ev: &InstallEvent) {
    match ev {
        InstallEvent::RetrievingPackages { num, total_bytes } => {
            state.download_total = *num;
            state.download_done = 0;
            state.download_bytes_total = *total_bytes;
            state.download_bytes_done = 0;
            state.download_files.clear();
        }
        InstallEvent::DownloadProgress {
            filename,
            downloaded,
            ..
        } => {
            let prev = state
                .download_files
                .insert(filename.clone(), *downloaded)
                .unwrap_or(0);
            state.download_bytes_done += *downloaded - prev;
        }
        InstallEvent::DownloadRetry { filename, resume } => {
            if !*resume && let Some(prev) = state.download_files.remove(filename) {
                state.download_bytes_done -= prev;
            }
        }
        InstallEvent::DownloadCompleted { .. } => {
            state.download_done += 1;
        }
        _ => {}
    }
}

fn event_stage(ev: &InstallEvent) -> Option<RepoStage> {
    use InstallEvent::*;
    use RepoStage::*;
    match ev {
        ResolvingDependencies => Some(Resolve),
        CheckingConflicts
        | CheckingFileConflicts
        | CheckingIntegrity
        | CheckingDiskSpace
        | LoadingPackages
        | KeyringStart => Some(Validate),
        Progress { phase, .. } => match phase {
            ProgressPhase::Conflicts
            | ProgressPhase::Diskspace
            | ProgressPhase::Integrity
            | ProgressPhase::Load
            | ProgressPhase::Keyring => Some(Validate),
            ProgressPhase::Add
            | ProgressPhase::Upgrade
            | ProgressPhase::Downgrade
            | ProgressPhase::Reinstall
            | ProgressPhase::Remove => Some(Install),
        },
        RetrievingPackages { .. }
        | DownloadInit { .. }
        | DownloadProgress { .. }
        | DownloadRetry { .. }
        | DownloadCompleted { .. } => Some(Download),
        PackageOperation { .. } => Some(Install),
        HookRun { .. } | ScriptletInfo { .. } | TransactionDone => Some(Finalize),
        _ => None,
    }
}

fn ordered_aur_stages() -> &'static [AurStage] {
    use AurStage::*;
    &[Resolve, Build, Validate, Install, Finalize]
}

fn aur_event_stage(ev: &InstallEvent) -> Option<AurStage> {
    use AurStage::*;
    use InstallEvent::*;
    match ev {
        ResolvingAurDependencies { .. }
        | AurDepResolved { .. }
        | ResolutionComplete { .. }
        | LayerBoundary { .. } => Some(Resolve),
        CloningRepo { .. } | BuildStarted { .. } | BuildOutput { .. } | BuildCompleted { .. } => {
            Some(Build)
        }
        LoadingPackages
        | ResolvingDependencies
        | CheckingConflicts
        | CheckingFileConflicts
        | CheckingIntegrity
        | CheckingDiskSpace
        | KeyringStart => Some(Validate),
        ProcessingChanges | PackageOperation { .. } => Some(Install),
        Progress { phase, .. } => match phase {
            ProgressPhase::Conflicts
            | ProgressPhase::Diskspace
            | ProgressPhase::Integrity
            | ProgressPhase::Load
            | ProgressPhase::Keyring => Some(Validate),
            ProgressPhase::Add
            | ProgressPhase::Upgrade
            | ProgressPhase::Downgrade
            | ProgressPhase::Reinstall
            | ProgressPhase::Remove => Some(Install),
        },
        TransactionDone | HookRun { .. } | ScriptletInfo { .. } => Some(Finalize),
        _ => None,
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

impl Render for InstallPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(self.mode, PageMode::Aur(_)) {
            self.render_aur(cx)
        } else {
            self.render_repo(cx)
        }
    }
}
