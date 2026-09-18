use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);
static THEME_SET: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
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

pub fn parse_unified_diff(input: &str) -> Vec<DiffLine> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledSegment {
    pub color: RgbColor,
    pub text: String,
    pub emphasized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderedLine {
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

pub fn shell_syntax() -> &'static SyntaxReference {
    SYNTAX_SET
        .find_syntax_by_name("Bourne Again Shell (bash)")
        .or_else(|| SYNTAX_SET.find_syntax_by_extension("sh"))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

pub fn syntax_for_file(name: &str) -> &'static SyntaxReference {
    if name == "PKGBUILD" || name.ends_with(".install") || name.ends_with(".sh") {
        return shell_syntax();
    }
    SYNTAX_SET
        .find_syntax_for_file(name)
        .ok()
        .flatten()
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

pub fn highlight_line(body: &str, highlighter: &mut HighlightLines) -> Vec<StyledSegment> {
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
                color: RgbColor {
                    r: foreground.r,
                    g: foreground.g,
                    b: foreground.b,
                    a: foreground.a,
                },
                text: fragment.to_owned(),
                emphasized: false,
            }
        })
        .collect()
}

pub type Fragment = Option<(usize, usize)>;

pub fn fragment_pair(removed: &str, added: &str) -> (Fragment, Fragment) {
    let removed: Vec<char> = removed.chars().collect();
    let added: Vec<char> = added.chars().collect();
    let prefix = removed
        .iter()
        .zip(&added)
        .take_while(|(l, r)| l == r)
        .count();
    let max_suffix = (removed.len() - prefix).min(added.len() - prefix);
    let suffix = removed
        .iter()
        .rev()
        .zip(added.iter().rev())
        .take(max_suffix)
        .take_while(|(l, r)| l == r)
        .count();
    let range = |len: usize| {
        if len <= prefix + suffix {
            None
        } else if prefix == 0 && suffix == 0 {
            Some((0, usize::MAX))
        } else {
            Some((prefix, len - suffix))
        }
    };
    (range(removed.len()), range(added.len()))
}

fn flush_fragments(
    emphasis: &mut [Option<(usize, usize)>],
    lines: &[DiffLine],
    removed: &mut Vec<usize>,
    added: &mut Vec<usize>,
    full: bool,
) {
    for (removed_index, added_index) in removed.iter().zip(added.iter()) {
        if let (DiffLine::Removed(removed_text), DiffLine::Added(added_text)) =
            (&lines[*removed_index], &lines[*added_index])
        {
            (emphasis[*removed_index], emphasis[*added_index]) =
                fragment_pair(removed_text, added_text);
        }
    }
    if full {
        let paired = removed.len().min(added.len());
        for leftover in removed.iter().skip(paired).chain(added.iter().skip(paired)) {
            emphasis[*leftover] = Some((0, usize::MAX));
        }
    }
    removed.clear();
    added.clear();
}

pub fn pair_fragments(lines: &[DiffLine]) -> Vec<Option<(usize, usize)>> {
    let mut emphasis = vec![None; lines.len()];
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut seen_removed = false;
    for (index, line) in lines.iter().enumerate() {
        match line {
            DiffLine::Removed(_) => {
                if !added.is_empty() {
                    flush_fragments(&mut emphasis, lines, &mut removed, &mut added, seen_removed);
                }
                removed.push(index);
                seen_removed = true;
            }
            DiffLine::Added(_) => added.push(index),
            _ => {
                flush_fragments(&mut emphasis, lines, &mut removed, &mut added, seen_removed);
                if matches!(line, DiffLine::FileHeader { .. }) {
                    seen_removed = false;
                }
            }
        }
    }
    flush_fragments(&mut emphasis, lines, &mut removed, &mut added, seen_removed);
    emphasis
}

pub fn highlight_fragmented(
    body: &str,
    fragment: Option<(usize, usize)>,
    highlighter: &mut HighlightLines,
) -> Vec<StyledSegment> {
    let Some((start, end)) = fragment else {
        return highlight_line(body, highlighter);
    };
    let len = body.chars().count();
    let (start, end) = (start.min(len), end.min(len));
    if start >= end {
        return highlight_line(body, highlighter);
    }
    let byte = |nth: usize| {
        body.char_indices()
            .map(|(index, _)| index)
            .chain([body.len()])
            .nth(nth)
            .unwrap_or(body.len())
    };
    let (head, tail) = (byte(start), byte(end));
    let mut segments = highlight_line(&body[..head], highlighter);
    segments.extend(
        highlight_line(&body[head..tail], highlighter)
            .into_iter()
            .map(|segment| StyledSegment {
                emphasized: true,
                ..segment
            }),
    );
    segments.extend(highlight_line(&body[tail..], highlighter));
    segments
}

pub fn rendered_lines(lines: &[DiffLine], is_new: bool, dark: bool) -> Vec<RenderedLine> {
    let theme = THEME_SET.get(if dark {
        EmbeddedThemeName::Base16OceanDark
    } else {
        EmbeddedThemeName::InspiredGithub
    });
    let plain = SYNTAX_SET.find_syntax_plain_text();
    let mut highlighter = HighlightLines::new(plain, theme);
    let emphasis = if is_new {
        vec![None; lines.len()]
    } else {
        pair_fragments(lines)
    };
    lines
        .iter()
        .zip(emphasis)
        .filter_map(|(line, line_emphasis)| match line {
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
                segments: highlight_fragmented(body, line_emphasis, &mut highlighter),
            }),
            DiffLine::Removed(body) => Some(RenderedLine::Code {
                marker: if is_new { ' ' } else { '-' },
                text: body.clone(),
                segments: highlight_fragmented(body, line_emphasis, &mut highlighter),
            }),
            DiffLine::Context(body) => Some(RenderedLine::Code {
                marker: ' ',
                text: body.clone(),
                segments: highlight_line(body, &mut highlighter),
            }),
        })
        .collect()
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
        rendered_lines(&parse_unified_diff(input), is_new, true)
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
        let rows = rendered_lines(&parse_unified_diff(single), false, true);
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

    fn diff_input(hunk: &str, body: &str) -> String {
        format!("diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n{hunk}{body}")
    }

    fn fragments(input: &str) -> Vec<Option<(usize, usize)>> {
        pair_fragments(&parse_unified_diff(input))
    }

    const FULL: Option<(usize, usize)> = Some((0, usize::MAX));

    #[test]
    fn changed_fragments_pair_and_trim() {
        let cases = [
            ("-pkgver=1\n+pkgver=2\n", vec![Some((7, 8)), Some((7, 8))]),
            (
                "-foo old bar\n+foo new bar\n",
                vec![Some((4, 7)), Some((4, 7))],
            ),
            ("-same\n+same\n", vec![None, None]),
            (
                "-old1\n-old2\n+new1\n+new2\n",
                vec![Some((0, 3)), Some((0, 3)), Some((0, 3)), Some((0, 3))],
            ),
            ("-foo\n+\n", vec![FULL, None]),
            (
                "-café old bar\n+café new bar\n",
                vec![Some((5, 8)), Some((5, 8))],
            ),
        ];
        for (body, expected) in cases {
            assert_eq!(
                &fragments(&diff_input("@@ -1,1 +1,1 @@\n", body))[2..],
                expected
            );
        }
    }

    #[test]
    fn fragments_stay_local_to_each_run() {
        assert_eq!(
            &fragments(&diff_input("@@ -1,2 +1,1\n", "-gone\n keep\n"))[2..],
            [FULL, None]
        );
        let split = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -1,1 +1,1 @@\n-gone\n",
            "@@ -2,1 +2,1 @@\n+here\n",
        );
        assert_eq!(fragments(split), vec![None, None, FULL, None, FULL]);
        let mixed = concat!(
            "diff --git a/new.install b/new.install\n--- a/new.install\n+++ b/new.install\n",
            "@@ -0,0 +1,3 @@\n+one\n+two\n+three\n",
            "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
            "@@ -1,1 +1,2 @@\n-old\n+new\n+extra\n",
        );
        let mixed = fragments(mixed);
        assert_eq!(&mixed[2..5], [None, None, None]);
        assert_eq!(&mixed[7..], [FULL, FULL, FULL]);
    }

    #[test]
    fn fragmented_highlight_marks_only_the_middle() {
        let theme = THEME_SET.get(EmbeddedThemeName::Base16OceanDark);
        let mut highlighter = HighlightLines::new(SYNTAX_SET.find_syntax_plain_text(), theme);
        let segments = highlight_fragmented("café old bar", Some((5, 8)), &mut highlighter);
        assert_eq!(
            segments
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "café old bar"
        );
        assert_eq!(
            segments
                .iter()
                .filter(|part| part.emphasized)
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "old"
        );
    }
}
