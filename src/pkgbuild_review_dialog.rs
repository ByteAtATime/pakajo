use std::sync::Arc;

use crate::pkgbuild::{PkgbuildDiff, mark_seen};
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    spinner::Spinner,
    v_flex,
};

type OnCompleteFn = Box<dyn FnOnce(&mut Window, &mut App) + 'static>;
type CancelFn = Arc<dyn Fn(&mut Window, &mut App) + 'static>;

pub struct PkgbuildReviewFlow {
    dialog: Option<Entity<PkgbuildReviewDialog>>,
}

impl PkgbuildReviewFlow {
    pub fn new() -> Self {
        Self { dialog: None }
    }

    pub fn begin(
        &mut self,
        on_complete: OnCompleteFn,
        on_cancel: CancelFn,
        window: &mut Window,
        cx: &mut App,
    ) {
        let on_cancel_for_dialog = on_cancel.clone();
        let dialog = cx.new(|_| PkgbuildReviewDialog::loading(on_complete, on_cancel));
        self.dialog = Some(dialog.clone());

        window.open_dialog(cx, move |d, _window, cx| {
            let title = build_title(cx);
            let on_cancel_arc = on_cancel_for_dialog.clone();
            let dialog_for_content = dialog.clone();
            d.title(title)
                .w(px(720.))
                .close_button(true)
                .on_cancel(move |_, window, cx| {
                    on_cancel_arc(window, cx);
                    false
                })
                .content(move |content, _, _| content.child(dialog_for_content.clone()))
        });
    }

    pub fn show_review(&mut self, diffs: Vec<PkgbuildDiff>, cx: &mut App) {
        let Some(dialog) = &self.dialog else {
            return;
        };
        dialog.update(cx, |d, cx| {
            d.start_review(diffs);
            cx.notify();
        });
    }

    pub fn end(&mut self, window: &mut Window, cx: &mut App) {
        if self.dialog.take().is_some() {
            window.close_dialog(cx);
        }
    }

    pub fn is_active(&self) -> bool {
        self.dialog.is_some()
    }
}

enum ReviewState {
    Loading,
    Ready,
}

pub struct PkgbuildReviewDialog {
    state: ReviewState,
    diffs: Vec<PkgbuildDiff>,
    current_index: usize,
    accepted: Vec<bool>,
    on_complete: Option<OnCompleteFn>,
    on_cancel: CancelFn,
}

impl PkgbuildReviewDialog {
    pub fn loading(on_complete: OnCompleteFn, on_cancel: CancelFn) -> Self {
        Self {
            state: ReviewState::Loading,
            diffs: Vec::new(),
            current_index: 0,
            accepted: Vec::new(),
            on_complete: Some(on_complete),
            on_cancel,
        }
    }

    pub fn start_review(&mut self, diffs: Vec<PkgbuildDiff>) {
        self.state = ReviewState::Ready;
        self.accepted = vec![false; diffs.len()];
        self.diffs = diffs;
    }

    fn remaining_after_current(&self) -> usize {
        self.accepted
            .iter()
            .enumerate()
            .filter(|(i, accepted)| !**accepted && *i != self.current_index)
            .count()
    }

    fn next_unaccepted_index(&self) -> usize {
        let total = self.accepted.len();
        for offset in 1..=total {
            let idx = (self.current_index + offset) % total;
            if !self.accepted[idx] {
                return idx;
            }
        }
        self.current_index
    }

    pub fn accept_current(&mut self, cx: &mut Context<Self>) -> Option<OnCompleteFn> {
        if self.diffs.is_empty() {
            return None;
        }
        self.accepted[self.current_index] = true;
        let _ = mark_seen(&self.diffs[self.current_index].dir);
        if !self.accepted.iter().all(|&accepted| accepted) {
            self.current_index = self.next_unaccepted_index();
            cx.notify();
            return None;
        }
        self.on_complete.take()
    }

    fn render_loading(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w_full()
            .gap_3()
            .items_center()
            .py_4()
            .child(Spinner::new())
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading PKGBUILD"),
            )
    }

    fn render_review(&self, cx: &mut Context<Self>) -> Div {
        let primary = cx.theme().primary;
        let muted = cx.theme().muted;
        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;
        let green = cx.theme().green;
        let danger = cx.theme().danger;
        let border = cx.theme().border;

        let current_index = self.current_index;
        let total = self.diffs.len();
        let current = self.diffs[current_index].clone();
        let is_new = current.is_new;
        let status_word = if is_new { "new" } else { "updated" };
        let accepted_flags = self.accepted.clone();
        let remaining = self.remaining_after_current();
        let on_cancel = self.on_cancel.clone();
        let entity = cx.entity();

        let accept_label = if remaining == 0 {
            "Accept & Build"
        } else {
            "Accept & Continue"
        };

        let tabs = self
            .diffs
            .iter()
            .enumerate()
            .map(|(index, diff)| {
                let is_active = index == current_index;
                let is_accepted = accepted_flags[index];
                let bg = if is_active {
                    primary.opacity(0.15)
                } else {
                    muted
                };
                let fg = if is_active {
                    foreground
                } else {
                    muted_foreground
                };
                let mut tab = h_flex()
                    .id(("pkgbuild-tab", index))
                    .gap_1()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.current_index = index;
                        cx.notify();
                    }))
                    .child(div().child(diff.name.clone()));
                if is_accepted {
                    tab = tab.child(Icon::new(IconName::Check).text_color(green));
                }
                tab.into_any_element()
            })
            .collect::<Vec<_>>();

        let diff_lines = current
            .diff
            .lines()
            .map(|line| {
                let color = diff_line_color(line, primary, green, danger, muted_foreground);
                (line.to_string(), color)
            })
            .collect::<Vec<_>>();

        let footer = h_flex()
            .justify_between()
            .child(
                Button::new("pkgbuild-cancel")
                    .label("Cancel")
                    .ghost()
                    .rounded_none()
                    .large()
                    .on_click(move |_, window, cx| {
                        on_cancel(window, cx);
                    }),
            )
            .child(
                Button::new("pkgbuild-accept")
                    .label(accept_label)
                    .primary()
                    .rounded_none()
                    .large()
                    .icon(IconName::ArrowRight)
                    .on_click(move |_, window, cx| {
                        let on_complete = entity.update(cx, |this, cx| this.accept_current(cx));
                        if let Some(on_complete) = on_complete {
                            on_complete(window, cx);
                        }
                    }),
            );

        let tab_bar = if total > 1 {
            Some(h_flex().gap_1().children(tabs).into_any_element())
        } else {
            None
        };

        let progress_label = if total > 1 {
            Some(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(format!(
                        "Package {} of {} — {} ({})",
                        current_index + 1,
                        total,
                        current.name,
                        status_word,
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };

        v_flex()
            .w_full()
            .gap_3()
            .children(tab_bar)
            .children(progress_label)
            .child(
                div()
                    .id("pkgbuild-diff-scroll")
                    .flex_1()
                    .min_h_0()
                    .max_h(px(480.))
                    .overflow_y_scroll()
                    .bg(muted)
                    .p_3()
                    .v_flex()
                    .gap_0()
                    .font_family("JetBrains Mono")
                    .text_size(rems(0.8))
                    .children(
                        diff_lines
                            .into_iter()
                            .map(|(text, color)| div().text_color(color).child(text)),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
            .child(footer)
    }
}

fn diff_line_color(line: &str, primary: Hsla, green: Hsla, danger: Hsla, muted: Hsla) -> Hsla {
    if line.starts_with('+') {
        green
    } else if line.starts_with('-') {
        danger
    } else if line.starts_with("@@") {
        primary
    } else {
        muted
    }
}

impl Render for PkgbuildReviewDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.state {
            ReviewState::Loading => self.render_loading(cx),
            ReviewState::Ready => self.render_review(cx),
        }
    }
}

pub fn build_title(_cx: &App) -> impl IntoElement {
    div().text_lg().font_semibold().child("Review PKGBUILD")
}
