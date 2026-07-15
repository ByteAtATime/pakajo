use gpui::*;
use gpui_component::{ActiveTheme as _, StyledExt as _};

pub struct InstallLogOverlay {
    pub logs: Vec<String>,
}

impl Render for InstallLogOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("install-log-scroll")
            .overflow_y_scroll()
            .max_h(px(400.))
            .v_flex()
            .gap_1()
            .p_3()
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .text_size(rems(0.8))
            .children(self.logs.iter().cloned().map(|line| div().child(line)))
    }
}
