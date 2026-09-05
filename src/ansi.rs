use crate::color::ANSI_RE;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct AnsiStyle {
    pub fg: Option<AnsiColor>,
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnsiColor {
    Basic(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StyledSpan {
    pub text: String,
    pub style: AnsiStyle,
}

fn extended(params: &[u16], i: usize) -> (usize, Option<AnsiColor>) {
    if i + 1 >= params.len() {
        return (1, None);
    }
    let q = |j: usize| params[i + j].min(255) as u8;
    if params[i + 1] == 5 {
        if i + 2 >= params.len() {
            return (params.len() - i, None);
        }
        return (3, Some(AnsiColor::Indexed(q(2))));
    }
    if params[i + 1] == 2 {
        if i + 4 >= params.len() {
            return (params.len() - i, None);
        }
        return (5, Some(AnsiColor::Rgb(q(2), q(3), q(4))));
    }
    (1, None)
}

fn apply_sgr(params: &str, style: &mut AnsiStyle) {
    let codes: Vec<u16> = params
        .split(';')
        .filter_map(|segment| {
            let head = segment.split(':').next().unwrap_or("");
            if head.is_empty() {
                return Some(0);
            }
            head.parse().ok()
        })
        .collect();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = AnsiStyle::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            4 => style.underline = true,
            9 => style.strikethrough = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            24 => style.underline = false,
            29 => style.strikethrough = false,
            30..=37 => style.fg = Some(AnsiColor::Basic((codes[i] - 30) as u8)),
            39 => style.fg = None,
            90..=97 => style.fg = Some(AnsiColor::Basic((codes[i] - 90 + 8) as u8)),
            38 | 48 | 58 => {
                let (consumed, color) = extended(&codes, i);
                if codes[i] == 38 && color.is_some() {
                    style.fg = color;
                }
                i += consumed;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

fn push_run(spans: &mut Vec<StyledSpan>, text: &str, style: AnsiStyle) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
        return;
    }
    spans.push(StyledSpan {
        text: text.to_string(),
        style,
    });
}

pub fn parse_line(line: &str) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut style = AnsiStyle::default();
    let mut prev = 0;
    for hit in ANSI_RE.find_iter(line) {
        push_run(&mut spans, &line[prev..hit.start()], style);
        let esc = hit.as_str();
        if esc.starts_with("\x1b[") && esc.ends_with('m') {
            apply_sgr(&esc[2..esc.len() - 1], &mut style);
        }
        prev = hit.end();
    }
    push_run(&mut spans, &line[prev..], style);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use AnsiColor::*;

    fn fl(fg: Option<AnsiColor>, bold: bool, dim: bool) -> AnsiStyle {
        AnsiStyle {
            fg,
            bold,
            dim,
            ..Default::default()
        }
    }

    fn styled(text: &str, style: AnsiStyle) -> StyledSpan {
        StyledSpan {
            text: text.into(),
            style,
        }
    }

    fn plain(text: &str) -> StyledSpan {
        styled(text, AnsiStyle::default())
    }

    fn fg_span(text: &str, color: AnsiColor) -> StyledSpan {
        styled(text, fl(Some(color), false, false))
    }

    #[test]
    fn parse_ansi() {
        let cases = [
            ("hello", vec![plain("hello")]),
            (
                "\x1b[1m==>\x1b[0m pkg",
                vec![styled("==>", fl(None, true, false)), plain(" pkg")],
            ),
            (
                "\x1b[1ma\x1b[mb",
                vec![styled("a", fl(None, true, false)), plain("b")],
            ),
            ("\x1b[91mx", vec![fg_span("x", Basic(9))]),
            ("\x1b[38;5;196mx", vec![fg_span("x", Indexed(196))]),
            ("\x1b[38;2;10;20;30mx", vec![fg_span("x", Rgb(10, 20, 30))]),
            ("\x1b[48;5;100;31mred", vec![fg_span("red", Basic(1))]),
            ("\x1b[58;5;7;31mred", vec![fg_span("red", Basic(1))]),
            (
                "\x1b[1;2mx\x1b[22my",
                vec![styled("x", fl(None, true, true)), plain("y")],
            ),
            (
                "\x1b[1;31mhi\x1b[39mbye",
                vec![
                    styled("hi", fl(Some(Basic(1)), true, false)),
                    styled("bye", fl(None, true, false)),
                ],
            ),
            ("\x1b[38:5:196mX", vec![plain("X")]),
            (
                "\x1b[1;?25;31mred",
                vec![styled("red", fl(Some(Basic(1)), true, false))],
            ),
            (
                "\x1b[1;4:3mx",
                vec![styled(
                    "x",
                    AnsiStyle {
                        bold: true,
                        underline: true,
                        ..Default::default()
                    },
                )],
            ),
            ("\x1b[38:2::1:2:3;31mred", vec![fg_span("red", Basic(1))]),
            ("\x1b[2Ktext", vec![plain("text")]),
            ("\x1b]0;t\x07hi", vec![plain("hi")]),
            ("a\x1b[2Kb", vec![plain("ab")]),
            ("\x1b[1m\x1b[0m", vec![]),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_line(input), expected, "input {input:?}");
        }
    }
}
