pub mod detail;
pub mod icons;
pub mod search;
pub mod sysupgrade;
pub(crate) mod theme;
pub mod transaction;
pub mod updates;

pub fn row_title<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::widget::text::Text<'a, cosmic::Theme, cosmic::Renderer> {
    cosmic::widget::text::body(content).font(cosmic::font::semibold())
}
