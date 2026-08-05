use std::sync::Arc;

use crate::session::GroupMember;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    v_flex,
};

type GroupApproveFn = Box<dyn FnOnce(Vec<String>, &mut Window, &mut App) + 'static>;
type GroupCancelFn = Arc<dyn Fn(&mut Window, &mut App) + 'static>;

pub struct GroupSelectDialog {
    members: Vec<GroupMember>,
    checks: Vec<bool>,
    action_label: String,
    on_approve: Option<GroupApproveFn>,
    on_cancel: GroupCancelFn,
}

impl GroupSelectDialog {
    pub fn new(
        members: Vec<GroupMember>,
        action_label: String,
        on_approve: GroupApproveFn,
        on_cancel: GroupCancelFn,
    ) -> Self {
        let checks = vec![true; members.len()];
        Self {
            members,
            checks,
            action_label,
            on_approve: Some(on_approve),
            on_cancel,
        }
    }

    fn any_checked(&self) -> bool {
        self.checks.iter().any(|&c| c)
    }

    fn selected_count(&self) -> usize {
        self.checks.iter().filter(|&&c| c).count()
    }

    fn render_member_row(
        &self,
        index: usize,
        m: &GroupMember,
        cx: &mut Context<Self>,
    ) -> Checkbox {
        let mut secondary = v_flex();
        let mut has_content = false;
        if let Some(d) = &m.description {
            secondary = secondary.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(d.clone()),
            );
            has_content = true;
        }
        if m.installed {
            secondary = secondary.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::Check).text_color(cx.theme().green))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("installed"),
                    ),
            );
            has_content = true;
        }
        let mut checkbox = Checkbox::new(("member", index))
            .checked(self.checks[index])
            .label(m.name.clone());
        if has_content {
            checkbox = checkbox.child(secondary);
        }
        checkbox
            .cursor_pointer()
            .on_click(cx.listener(move |this, &next: &bool, _w, cx| {
                if let Some(slot) = this.checks.get_mut(index) {
                    *slot = next;
                    cx.notify();
                }
            }))
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let approve_disabled = !self.any_checked();
        let approve_label = format!("{} {} packages", self.action_label, self.selected_count());
        let on_cancel = self.on_cancel.clone();
        let entity = cx.entity();
        h_flex()
            .justify_between()
            .child(
                Button::new("group-select-cancel")
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
                Button::new("group-select-approve")
                    .label(approve_label)
                    .primary()
                    .rounded_none()
                    .large()
                    .disabled(approve_disabled)
                    .icon(IconName::ArrowRight)
                    .on_click(move |_, window, cx| {
                        let action = entity.update(cx, |this, _cx| {
                            if this.any_checked() {
                                let names: Vec<String> = this
                                    .members
                                    .iter()
                                    .zip(this.checks.iter())
                                    .filter_map(|(m, &on)| {
                                        if on { Some(m.name.clone()) } else { None }
                                    })
                                    .collect();
                                let on_approve = this.on_approve.take();
                                Some((names, on_approve))
                            } else {
                                None
                            }
                        });
                        if let Some((names, Some(on_approve))) = action {
                            on_approve(names, window, cx);
                        }
                    }),
            )
    }
}

impl Render for GroupSelectDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = div()
            .id("group-select-scroll")
            .overflow_y_scroll()
            .max_h(px(400.))
            .v_flex()
            .gap_2()
            .children(
                self.members
                    .iter()
                    .enumerate()
                    .map(|(index, m)| self.render_member_row(index, m, cx)),
            );
        let footer = self.render_footer(cx);

        v_flex()
            .w_full()
            .gap_4()
            .child(scroll)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(footer)
    }
}
