use std::collections::HashMap;
use std::sync::Arc;

use crate::icon::PakajoIcon;
use crate::question::{Approvals, Conflict, ProviderCandidate, ProviderPrompt, QuestionSet};
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    radio::RadioGroup,
    v_flex,
};

type ApproveFn = Box<dyn FnOnce(Approvals, &mut Window, &mut App) + 'static>;
type CancelFn = Arc<dyn Fn(&mut Window, &mut App) + 'static>;

pub struct InstallReviewDialog {
    qs: QuestionSet,
    conflict_checks: Vec<bool>,
    provider_choices: HashMap<String, usize>,
    on_approve: Option<ApproveFn>,
    on_cancel: CancelFn,
}

impl InstallReviewDialog {
    pub fn new(qs: QuestionSet, on_approve: ApproveFn, on_cancel: CancelFn) -> Self {
        let conflict_checks = vec![false; qs.conflicts.len()];
        let provider_choices = qs
            .providers
            .iter()
            .map(|prompt| (prompt.depend.clone(), 0))
            .collect();
        Self {
            qs,
            conflict_checks,
            provider_choices,
            on_approve: Some(on_approve),
            on_cancel,
        }
    }

    fn any_conflict_unchecked(&self) -> bool {
        self.conflict_checks.iter().any(|&checked| !checked)
    }

    fn collect_approvals(&self) -> anyhow::Result<Approvals> {
        let conflict_selections: Vec<usize> = self
            .conflict_checks
            .iter()
            .enumerate()
            .filter_map(|(i, &on)| if on { Some(i) } else { None })
            .collect();

        let provider_choices: Vec<(usize, usize)> = self
            .qs
            .providers
            .iter()
            .enumerate()
            .filter_map(|(prompt_index, prompt)| {
                let &candidate_index = self.provider_choices.get(&prompt.depend)?;
                Some((prompt_index, candidate_index))
            })
            .collect();

        self.qs.approve(&conflict_selections, &provider_choices)
    }

    fn render_section_heading(&self, cx: &mut Context<Self>, color: Hsla, label: &str) -> Div {
        h_flex()
            .items_center()
            .gap_2()
            .child(div().size_2().rounded_full().bg(color))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
    }

    fn render_replacements_section(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_3()
            .child(self.render_section_heading(cx, cx.theme().danger, "REPLACEMENTS"))
            .child(div().h_px().w_full().bg(cx.theme().border))
            .children(
                self.qs
                    .conflicts
                    .iter()
                    .enumerate()
                    .map(|(index, conflict)| self.render_conflict_row(index, conflict, cx)),
            )
    }

    fn render_conflict_row(
        &self,
        index: usize,
        conflict: &Conflict,
        cx: &mut Context<Self>,
    ) -> Checkbox {
        let checked = self.conflict_checks[index];
        Checkbox::new(("conflict", index))
            .checked(checked)
            .label(format!(
                "Replace {} with {}",
                conflict.removable, conflict.incoming
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} will be removed", conflict.removable)),
            )
            .cursor_pointer()
            .on_click(cx.listener(move |this, &next_checked: &bool, _window, cx| {
                if let Some(slot) = this.conflict_checks.get_mut(index) {
                    *slot = next_checked;
                    cx.notify();
                }
            }))
    }

    fn render_providers_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.qs.providers.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .gap_3()
                .child(self.render_section_heading(cx, cx.theme().primary, "PROVIDERS"))
                .child(div().h_px().w_full().bg(cx.theme().border))
                .children(
                    self.qs
                        .providers
                        .iter()
                        .enumerate()
                        .map(|(index, prompt)| self.render_provider_prompt(index, prompt, cx)),
                ),
        )
    }

    fn render_provider_prompt(
        &self,
        index: usize,
        prompt: &ProviderPrompt,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = self
            .provider_choices
            .get(&prompt.depend)
            .copied()
            .or(Some(0));
        let depend = prompt.depend.clone();
        v_flex()
            .gap_2()
            .child(div().text_sm().font_semibold().child(prompt.depend.clone()))
            .child(
                RadioGroup::vertical(("provider", index))
                    .selected_index(selected)
                    .children(
                        prompt
                            .candidates
                            .iter()
                            .map(|candidate| candidate_label(candidate)),
                    )
                    .on_click(cx.listener(move |this, &chosen: &usize, _window, cx| {
                        this.provider_choices.insert(depend.clone(), chosen);
                        cx.notify();
                    })),
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let approve_disabled = self.any_conflict_unchecked();
        let on_cancel = self.on_cancel.clone();
        let entity = cx.entity();
        h_flex()
            .justify_between()
            .child(
                Button::new("review-cancel")
                    .label("Cancel")
                    .ghost()
                    .rounded_none()
                    .large()
                    .on_click(move |_, window, cx| {
                        on_cancel(window, cx);
                        window.close_dialog(cx);
                    }),
            )
            .child(
                Button::new("review-approve")
                    .label("Approve & install")
                    .primary()
                    .rounded_none()
                    .large()
                    .disabled(approve_disabled)
                    .icon(IconName::ArrowRight)
                    .on_click(move |_, window, cx| {
                        let action =
                            entity.update(cx, |this, _cx| match this.collect_approvals() {
                                Ok(approvals) => {
                                    let on_approve = this.on_approve.take();
                                    Some((approvals, on_approve))
                                }
                                Err(err) => {
                                    eprintln!(
                                        "[pakajo] failed to collect review approvals: {err:#}"
                                    );
                                    None
                                }
                            });
                        if let Some((approvals, Some(on_approve))) = action {
                            on_approve(approvals, window, cx);
                        }
                    }),
            )
    }
}

fn candidate_label(candidate: &ProviderCandidate) -> String {
    let qualified_name = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    match &candidate.version {
        Some(version) => format!("{qualified_name}  {version}"),
        None => qualified_name,
    }
}

impl Render for InstallReviewDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let replacements = self.render_replacements_section(cx);
        let providers = self.render_providers_section(cx);
        let footer = self.render_footer(cx);

        v_flex()
            .w_full()
            .gap_4()
            .child(
                div()
                    .id("install-review-scroll")
                    .overflow_y_scroll()
                    .max_h(px(400.))
                    .v_flex()
                    .gap_4()
                    .child(replacements)
                    .children(providers),
            )
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(footer)
    }
}

pub fn build_title(package_name: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(PakajoIcon::PackageCheck).text_color(cx.theme().primary))
        .child(
            div()
                .text_lg()
                .font_semibold()
                .child(format!("Installing {package_name}")),
        )
}
