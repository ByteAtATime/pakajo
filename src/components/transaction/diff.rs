use std::sync::LazyLock;

use cosmic::iced::widget::text::LineHeight;
use cosmic::iced::widget::{rich_text, span};
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, container, text};
use syntect::easy::HighlightLines;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

use super::shared::{BadgeColor, destructive_color, muted_color, success_color};
use crate::Element;
use crate::components::theme::muted;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);
static THEME_SET: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

type MonoSpan = cosmic::iced::widget::text::Span<'static, (), cosmic::font::Font>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiffLine {
    FileHeader {
        name: String,
        added: usize,
        removed: usize,
    },
    HunkMeta {
        old_start: usize,
        old_len: usize,
        new_start: usize,
        new_len: usize,
        elided: usize,
    },
    Added(String),
    Removed(String),
    Context(String),
}

fn parse_range(side: &str) -> Option<(usize, usize)> {
    let mut parts = side.trim_start_matches(['-', '+']).split(',');
    let start: usize = parts.next()?.parse().ok()?;
    let len = parts.next().map_or(Ok(1), |len| len.parse()).ok()?;
    Some((start, len))
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let mut words = line.strip_prefix("@@")?.split_whitespace();
    let (old_start, old_len) = parse_range(words.next()?)?;
    let (new_start, new_len) = parse_range(words.next()?)?;
    Some((old_start, old_len, new_start, new_len))
}

fn file_basename(line: &str) -> String {
    let path = line.split(' ').nth(2).unwrap_or(line);
    let path = path
        .strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn is_hidden_chrome(line: &str) -> bool {
    const PREFIXES: [&str; 11] = [
        "index ",
        "new file",
        "deleted file",
        "old mode",
        "new mode",
        "rename from",
        "rename to",
        "\\ No newline",
        "Binary files ",
        "similarity index",
        "dissimilarity index",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

fn is_file_marker(line: &str) -> bool {
    line.starts_with("--- ") || line == "---" || line.starts_with("+++ ") || line == "+++"
}

fn content_line(raw: &str) -> DiffLine {
    if let Some(body) = raw.strip_prefix('+') {
        DiffLine::Added(body.to_owned())
    } else if let Some(body) = raw.strip_prefix('-') {
        DiffLine::Removed(body.to_owned())
    } else if let Some(body) = raw.strip_prefix(' ') {
        DiffLine::Context(body.to_owned())
    } else {
        DiffLine::Context(raw.to_owned())
    }
}

enum Classified {
    Hunk(usize, usize, usize, usize),
    File,
    Hidden,
    Body,
}

fn classify(raw: &str, in_body: bool) -> Classified {
    if let Some((old_start, old_len, new_start, new_len)) = parse_hunk_header(raw) {
        return Classified::Hunk(old_start, old_len, new_start, new_len);
    }
    if raw.starts_with("diff --git ") {
        return Classified::File;
    }
    if is_hidden_chrome(raw) || (!in_body && is_file_marker(raw)) {
        return Classified::Hidden;
    }
    Classified::Body
}

pub(crate) fn parse_unified_diff(input: &str) -> Vec<DiffLine> {
    let mut sections: Vec<DiffLine> = Vec::new();
    let mut current: Option<usize> = None;
    let mut previous_new_end: Option<usize> = None;
    let mut body_remaining: usize = 0;
    for raw in input.lines() {
        match classify(raw, body_remaining > 0) {
            Classified::Hunk(old_start, old_len, new_start, new_len) => {
                let elided = previous_new_end.map_or(new_start.saturating_sub(1), |end| {
                    new_start.saturating_sub(end)
                });
                previous_new_end = Some(new_start + new_len);
                body_remaining = old_len + new_len;
                sections.push(DiffLine::HunkMeta {
                    old_start,
                    old_len,
                    new_start,
                    new_len,
                    elided,
                });
            }
            Classified::File => {
                sections.push(DiffLine::FileHeader {
                    name: file_basename(raw),
                    added: 0,
                    removed: 0,
                });
                current = Some(sections.len() - 1);
                previous_new_end = None;
                body_remaining = 0;
            }
            Classified::Hidden => {}
            Classified::Body => {
                let line = content_line(raw);
                body_remaining = body_remaining.saturating_sub(1);
                if let Some(DiffLine::FileHeader { added, removed, .. }) =
                    current.and_then(|index| sections.get_mut(index))
                {
                    match &line {
                        DiffLine::Added(_) => *added += 1,
                        DiffLine::Removed(_) => *removed += 1,
                        _ => {}
                    }
                }
                sections.push(line);
            }
        }
    }
    sections
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StyledSegment {
    pub(crate) color: Color,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RenderedLine {
    Code {
        marker: char,
        text: String,
        segments: Vec<StyledSegment>,
    },
    FileHeader {
        name: String,
        added: usize,
        removed: usize,
    },
    Gap {
        elided: usize,
    },
}

fn shell_syntax() -> &'static SyntaxReference {
    SYNTAX_SET
        .find_syntax_by_name("Bourne Again Shell (bash)")
        .or_else(|| SYNTAX_SET.find_syntax_by_extension("sh"))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

fn syntax_for_file(name: &str) -> &'static SyntaxReference {
    if name == "PKGBUILD" || name.ends_with(".install") || name.ends_with(".sh") {
        return shell_syntax();
    }
    SYNTAX_SET
        .find_syntax_for_file(name)
        .ok()
        .flatten()
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

fn highlight_line(body: &str, highlighter: &mut HighlightLines) -> Vec<StyledSegment> {
    if body.is_empty() {
        return Vec::new();
    }
    let Ok(ranges) = highlighter.highlight_line(body, &SYNTAX_SET) else {
        return Vec::new();
    };
    ranges
        .into_iter()
        .map(|(style, fragment)| {
            let foreground = style.foreground;
            StyledSegment {
                color: Color::from_rgba8(
                    foreground.r,
                    foreground.g,
                    foreground.b,
                    foreground.a as f32 / 255.0,
                ),
                text: fragment.to_owned(),
            }
        })
        .collect()
}

pub(crate) fn rendered_lines(lines: &[DiffLine], is_new: bool) -> Vec<RenderedLine> {
    let theme = THEME_SET.get(if cosmic::theme::is_dark() {
        EmbeddedThemeName::Base16OceanDark
    } else {
        EmbeddedThemeName::InspiredGithub
    });
    let plain = SYNTAX_SET.find_syntax_plain_text();
    let mut highlighter = HighlightLines::new(plain, theme);
    lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::FileHeader {
                name,
                added,
                removed,
            } => {
                highlighter = HighlightLines::new(syntax_for_file(name), theme);
                Some(RenderedLine::FileHeader {
                    name: name.clone(),
                    added: *added,
                    removed: *removed,
                })
            }
            DiffLine::HunkMeta { elided, .. } => {
                if is_new || *elided == 0 {
                    return None;
                }
                Some(RenderedLine::Gap { elided: *elided })
            }
            DiffLine::Added(body) => Some(RenderedLine::Code {
                marker: if is_new { ' ' } else { '+' },
                text: body.clone(),
                segments: highlight_line(body, &mut highlighter),
            }),
            DiffLine::Removed(body) => Some(RenderedLine::Code {
                marker: if is_new { ' ' } else { '-' },
                text: body.clone(),
                segments: highlight_line(body, &mut highlighter),
            }),
            DiffLine::Context(body) => Some(RenderedLine::Code {
                marker: ' ',
                text: body.clone(),
                segments: highlight_line(body, &mut highlighter),
            }),
        })
        .collect()
}

fn marker_color(theme: &cosmic::Theme, marker: char) -> Color {
    match marker {
        '+' => Color::from(theme.cosmic().success.base),
        '-' => Color::from(theme.cosmic().destructive.base),
        _ => muted_color(theme),
    }
}

fn code_spans(segments: &[StyledSegment], fallback: &str) -> Vec<MonoSpan> {
    if segments.is_empty() {
        return vec![
            span(fallback.to_owned())
                .font(cosmic::font::mono())
                .to_static(),
        ];
    }
    segments
        .iter()
        .map(|segment| {
            span(segment.text.clone())
                .color(segment.color)
                .font(cosmic::font::mono())
                .to_static()
        })
        .collect()
}

fn code_text(spans: Vec<MonoSpan>) -> Element<'static> {
    container(
        rich_text(spans)
            .font(cosmic::font::mono())
            .size(14.0)
            .line_height(LineHeight::Absolute(20.0.into())),
    )
    .width(Length::Fill)
    .into()
}

fn diff_row(marker: char, body: &str, segments: &[StyledSegment]) -> Element<'static> {
    let marker_cell = container(text::monotext(marker.to_string()))
        .width(Length::Fixed(16.0))
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(marker_color(theme, marker)),
            ..Default::default()
        });
    Row::new()
        .spacing(8)
        .push(marker_cell)
        .push(code_text(code_spans(segments, body)))
        .into()
}

fn count_chip(label: String, color: BadgeColor) -> Element<'static> {
    container(text(label))
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(color(theme)),
            ..Default::default()
        })
        .into()
}

fn file_header_element(name: String, added: usize, removed: usize) -> Element<'static> {
    let chip = container(text(name))
        .padding([2.0, 10.0])
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.16,
                ..muted_color(theme)
            })),
            border: Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let mut header = Row::new().spacing(8).push(chip);
    if added > 0 {
        header = header.push(count_chip(format!("+{added}"), success_color));
    }
    if removed > 0 {
        header = header.push(count_chip(format!("-{removed}"), destructive_color));
    }
    container(header)
        .padding([10.0, 0.0, 4.0, 0.0])
        .width(Length::Fill)
        .into()
}

fn gap_element(elided: usize) -> Element<'static> {
    muted(
        text(format!("<{} lines unchanged>", elided))
            .center()
            .width(Length::Fill),
    )
}

pub(crate) fn diff_rows_column(rows: &[RenderedLine], is_new: bool) -> Element<'_> {
    let mut column = Column::new().spacing(0);
    for rendered in rows {
        let row = match rendered {
            RenderedLine::FileHeader {
                name,
                added,
                removed,
            } => file_header_element(name.clone(), *added, *removed),
            RenderedLine::Gap { elided } => gap_element(*elided),
            RenderedLine::Code {
                marker,
                text,
                segments,
            } => {
                if is_new {
                    code_text(code_spans(segments, text))
                } else {
                    diff_row(*marker, text, segments)
                }
            }
        };
        column = column.push(row);
    }
    column.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_FILE: &str = concat!(
        "diff --git a/PKGBUILD b/PKGBUILD\nindex 1111111..2222222 100644\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
        "@@ -1,3 +1,4 @@\n line1\n-old1\n+new1\n+extra\n line3\n",
        "@@ -10,3 +11,3 @@\n line10\n-old10\n+new10\n line12\n",
        "diff --git a/foo.install b/foo.install\nindex 3333333..4444444 100644\n--- a/foo.install\n+++ b/foo.install\n",
        "@@ -1,2 +1,3 @@\n start\n+middle\n end\n",
    );

    fn headers(parsed: &[DiffLine]) -> Vec<(String, usize, usize)> {
        parsed
            .iter()
            .filter_map(|line| {
                let DiffLine::FileHeader {
                    name,
                    added,
                    removed,
                } = line
                else {
                    return None;
                };
                Some((name.clone(), *added, *removed))
            })
            .collect()
    }

    fn hunks(parsed: &[DiffLine]) -> Vec<(usize, usize, usize, usize, usize)> {
        parsed
            .iter()
            .filter_map(|line| {
                let DiffLine::HunkMeta {
                    old_start,
                    old_len,
                    new_start,
                    new_len,
                    elided,
                } = line
                else {
                    return None;
                };
                Some((*old_start, *old_len, *new_start, *new_len, *elided))
            })
            .collect()
    }

    fn bodies(parsed: &[DiffLine]) -> Vec<DiffLine> {
        parsed
            .iter()
            .filter(|line| {
                !matches!(
                    line,
                    DiffLine::FileHeader { .. } | DiffLine::HunkMeta { .. }
                )
            })
            .cloned()
            .collect()
    }

    #[test]
    fn multi_file_counts_and_elided_gaps() {
        let parsed = parse_unified_diff(MULTI_FILE);
        assert_eq!(
            headers(&parsed),
            vec![
                ("PKGBUILD".to_string(), 3, 2),
                ("foo.install".to_string(), 1, 0),
            ]
        );
        assert_eq!(
            hunks(&parsed),
            vec![(1, 3, 1, 4, 0), (10, 3, 11, 3, 6), (1, 2, 1, 3, 0),]
        );
    }

    #[test]
    fn hunk_header_edge_cases() {
        let offset = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -5,3 +8,3 @@\n line8\n-old8\n+new8\n",
        );
        assert_eq!(hunks(&parse_unified_diff(offset)), vec![(5, 3, 8, 3, 7)]);

        let bare = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -3 +3 @@\n same\n",
        );
        assert_eq!(hunks(&parse_unified_diff(bare)), vec![(3, 1, 3, 1, 2)]);
    }

    #[test]
    fn marker_like_content_inside_hunk_survives() {
        let raw = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -1,3 +1,4 @@\n line1\n--- foo\n+++ bar\n-# set file mode\n+extra\n",
        );
        assert_eq!(
            bodies(&parse_unified_diff(raw)),
            vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Removed("-- foo".to_string()),
                DiffLine::Added("++ bar".to_string()),
                DiffLine::Removed("# set file mode".to_string()),
                DiffLine::Added("extra".to_string()),
            ]
        );
    }

    #[test]
    fn chrome_never_surfaces_as_content() {
        let new_file = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/PKGBUILD\n",
            "@@ -0,0 +1,3 @@\n+pkgname=fixture\n+pkgver=1.0.0\n+arch=('x86_64')\n",
        );
        let parsed = parse_unified_diff(new_file);
        assert_eq!(hunks(&parsed), vec![(0, 0, 1, 3, 0)]);
        assert_eq!(
            bodies(&parsed),
            vec![
                DiffLine::Added("pkgname=fixture".to_string()),
                DiffLine::Added("pkgver=1.0.0".to_string()),
                DiffLine::Added("arch=('x86_64')".to_string()),
            ]
        );

        let renamed = concat!(
            "diff --git a/old-name b/new-name\nold mode 100644\nnew mode 100755\nrename from old-name\nrename to new-name\nindex 1111111..2222222\n--- a/old-name\n+++ b/new-name\n",
            "@@ -1,2 +1,2 @@\n keep\n-gone\n+here\n\\ No newline at end of file\nBinary files a/blob b/blob differ\n",
        );
        assert_eq!(
            bodies(&parse_unified_diff(renamed)),
            vec![
                DiffLine::Context("keep".to_string()),
                DiffLine::Removed("gone".to_string()),
                DiffLine::Added("here".to_string()),
            ]
        );
    }

    fn rendered_plain(input: &str, is_new: bool) -> Vec<String> {
        rendered_lines(&parse_unified_diff(input), is_new)
            .iter()
            .map(|line| match line {
                RenderedLine::Code { marker, text, .. } => format!("{marker}:{text}"),
                RenderedLine::FileHeader {
                    name,
                    added,
                    removed,
                } => format!("file:{name}+{added}-{removed}"),
                RenderedLine::Gap { elided } => format!("gap:{elided}"),
            })
            .collect()
    }

    fn expect_render(input: &str, is_new: bool, expected: &[&str]) {
        assert_eq!(rendered_plain(input, is_new), expected);
    }

    #[test]
    fn rendered_multi_file_sequence_with_gap() {
        expect_render(
            MULTI_FILE,
            false,
            &[
                "file:PKGBUILD+3-2",
                " :line1",
                "-:old1",
                "+:new1",
                "+:extra",
                " :line3",
                "gap:6",
                " :line10",
                "-:old10",
                "+:new10",
                " :line12",
                "file:foo.install+1-0",
                " :start",
                "+:middle",
                " :end",
            ],
        );
    }

    #[test]
    fn rendered_gap_only_for_elided_hunks() {
        let single = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -1,3 +1,4 @@\n line1\n-old1\n+new1\n+extra\n line3\n",
        );
        expect_render(
            single,
            false,
            &[
                "file:PKGBUILD+2-1",
                " :line1",
                "-:old1",
                "+:new1",
                "+:extra",
                " :line3",
            ],
        );
        let offset = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -5,3 +8,3 @@\n line8\n-old8\n+new8\n",
        );
        expect_render(
            offset,
            false,
            &["file:PKGBUILD+1-1", "gap:7", " :line8", "-:old8", "+:new8"],
        );
    }

    #[test]
    fn code_lines_carry_highlight_segments() {
        let single = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -1,1 +1,1 @@\n-pkgname=fixture\n",
        );
        let rows = rendered_lines(&parse_unified_diff(single), false);
        let code = rows.iter().find_map(|row| match row {
            RenderedLine::Code { text, segments, .. } => Some((text, segments)),
            _ => None,
        });
        let (text, segments) = code.expect("diff has one code line");
        assert_eq!(text, "pkgname=fixture");
        assert_eq!(
            segments
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "pkgname=fixture"
        );
    }

    #[test]
    fn shell_files_share_bash_syntax() {
        let pkgbuild = syntax_for_file("PKGBUILD").name.clone();
        assert_eq!(pkgbuild, syntax_for_file("hooks.install").name);
        assert_eq!(pkgbuild, syntax_for_file("setup.sh").name);
        assert_eq!(pkgbuild, "Bourne Again Shell (bash)");
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        assert_eq!(syntax_for_file("notes.unknownextxyz").name, "Plain Text");
    }
}
