use cosmic::iced::widget::text::{Rich, Span};
use cosmic::iced::{Color, Length};
use pakajo::ansi::{self, AnsiColor, StyledSpan};

use crate::Element;
use crate::components::theme::muted_color;

const DARK_PALETTE: [u32; 16] = [
    0x8b949e, 0xe5534b, 0x3fb950, 0xe3b341, 0x539bf5, 0xb083f0, 0x39c5cf, 0xd0d7de, 0xb0bac8,
    0xff7b72, 0x56d364, 0xf2cc60, 0x7aa2f7, 0xd2a8ff, 0x56d4dd, 0xf0f3f6,
];

const LIGHT_PALETTE: [u32; 16] = [
    0x24292f, 0xcf222e, 0x1a7f37, 0x9a6700, 0x0969da, 0x8250df, 0x1b7c83, 0x57606a, 0x6e7781,
    0xa40e26, 0x2da043, 0xbb8009, 0x218bff, 0xa475f4, 0x3192aa, 0x8c959f,
];

fn hex_color(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn indexed_color(n: u8, dark: bool) -> Color {
    if n < 16 {
        let palette = if dark { DARK_PALETTE } else { LIGHT_PALETTE };
        return hex_color(palette[n as usize]);
    }
    if n < 232 {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let v = n - 16;
        return Color::from_rgb8(
            LEVELS[(v / 36) as usize],
            LEVELS[((v % 36) / 6) as usize],
            LEVELS[(v % 6) as usize],
        );
    }
    let level = 8 + 10 * (n - 232);
    Color::from_rgb8(level, level, level)
}

fn ansi_color(color: AnsiColor, bold: bool, dim: bool, dark: bool) -> Color {
    let mut out = match color {
        AnsiColor::Basic(n) => indexed_color(if bold && n < 8 { n + 8 } else { n }, dark),
        AnsiColor::Indexed(n) => indexed_color(n, dark),
        AnsiColor::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
    };
    if dim {
        out.a *= 0.6;
    }
    out
}

fn tail_default_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(muted_color(theme)),
        ..Default::default()
    }
}

fn styled_span(span: StyledSpan, dark: bool) -> Span<'static> {
    let mut out = Span::new(span.text);
    if let Some(fg) = span.style.fg {
        out = out.color(ansi_color(fg, span.style.bold, span.style.dim, dark));
    }
    if span.style.bold {
        out = out.font(cosmic::iced::Font {
            weight: cosmic::iced::font::Weight::Bold,
            ..cosmic::font::mono()
        });
    }
    if span.style.underline {
        out = out.underline(true);
    }
    if span.style.strikethrough {
        out = out.strikethrough(true);
    }
    out
}

pub(crate) fn build_line_element(line: &str) -> Element<'static> {
    let dark = cosmic::theme::is_dark();
    let spans: Vec<Span<'static>> = ansi::parse_line(line)
        .into_iter()
        .map(|span| styled_span(span, dark))
        .collect();
    Rich::with_spans(spans)
        .font(cosmic::font::mono())
        .width(Length::Fill)
        .class(cosmic::theme::Text::Custom(tail_default_style))
        .into()
}
