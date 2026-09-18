use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::Context as _;

use syntect::easy::HighlightLines;

use crate::cli::prompts::confirm_review_accept;
use crate::color;
use crate::color::ColorTier;
use crate::diff::{DiffLine, StyledSegment, highlight_line, parse_unified_diff, syntax_for_file};
use crate::pkgbuild::{PkgbuildInfo, compute_diff};

pub fn review_pkgbuilds(pkgbuilds: &[PkgbuildInfo]) -> bool {
    let tier = color::terminal_tier();
    let enabled = tier != ColorTier::Off;
    let mut sections: Vec<(String, String)> = Vec::new();
    for pb in pkgbuilds {
        match compute_diff(&pb.dir, pb.is_new, false) {
            Ok(raw) => {
                if let Some(body) = render_package(&raw, pb.is_new, tier) {
                    sections.push((pb.name.clone(), body));
                }
            }
            Err(e) => {
                eprintln!("warning: failed to compute diff for {}: {e:#}", pb.name);
            }
        }
    }

    if sections.is_empty() {
        println!(
            "{}",
            color::colon(enabled, "Nothing new to review - all PKGBUILDs unchanged")
        );
        return true;
    }

    let mut combined = String::new();
    for (name, body) in &sections {
        combined.push_str(&color::colon(enabled, name));
        combined.push_str("\n\n");
        combined.push_str(body);
        combined.push_str("\n\n");
    }

    let pager = resolve_pager();
    let result = run_pager(&pager, &combined);
    if let Err(e) = result {
        eprintln!("warning: pager '{pager}' failed: {e:#}, printing to stdout");
        let _ = std::io::stdout().write_all(combined.as_bytes());
    }

    confirm_review_accept()
}

fn resolve_pager() -> String {
    if let Ok(p) = std::env::var("PAGER")
        && !p.is_empty()
    {
        return p;
    }
    if which("less") {
        "less".to_string()
    } else {
        "cat".to_string()
    }
}

fn which(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_pager(pager: &str, content: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(pager);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    if std::env::var("LESS").is_err() {
        cmd.env("LESS", "SRXF");
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn pager '{pager}'"))?;

    if let Some(stdin) = child.stdin.as_mut()
        && let Err(e) = stdin.write_all(content.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(anyhow::Error::from(e).context("failed to write to pager stdin"));
    }

    drop(child.stdin.take());

    let status = child.wait().context("failed to wait for pager")?;
    if !status.success() {
        eprintln!("warning: pager '{pager}' exited with non-zero status");
    }
    Ok(())
}

fn render_package(raw: &str, is_new: bool, tier: ColorTier) -> Option<String> {
    let lines = parse_unified_diff(raw);
    if !lines.iter().any(|line| {
        matches!(
            line,
            DiffLine::Added(_) | DiffLine::Removed(_) | DiffLine::Context(_)
        )
    }) {
        return None;
    }
    if is_new {
        return Some(render_new(&lines, tier));
    }
    Some(render_diff(&lines, tier))
}

fn paint_segment(segment: &StyledSegment, tier: ColorTier) -> String {
    match tier {
        ColorTier::Truecolor => format!(
            "\x1b[38;2;{};{};{}m{}\x1b[0m",
            segment.color.r, segment.color.g, segment.color.b, segment.text
        ),
        ColorTier::Basic16 => {
            let index = color::quantize_dark(segment.color.r, segment.color.g, segment.color.b);
            format!(
                "\x1b[{}m{}\x1b[0m",
                if index < 8 {
                    30 + index as u32
                } else {
                    82 + index as u32
                },
                segment.text
            )
        }
        ColorTier::Off => segment.text.clone(),
    }
}

fn new_highlighter(name: &str) -> HighlightLines<'_> {
    HighlightLines::new(syntax_for_file(name), crate::diff::terminal_theme())
}

fn render_code(text: &str, highlighter: Option<&mut HighlightLines>, tier: ColorTier) -> String {
    if tier == ColorTier::Off {
        return text.to_string();
    }
    let Some(highlighter) = highlighter else {
        return text.to_string();
    };
    highlight_line(text, highlighter)
        .iter()
        .map(|segment| paint_segment(segment, tier))
        .collect()
}

fn render_diff(lines: &[DiffLine], tier: ColorTier) -> String {
    let enabled = tier != ColorTier::Off;
    let added = color::paint(enabled, color::GREEN, "+");
    let removed = color::paint(enabled, color::RED, "-");
    let mut highlighter: Option<HighlightLines> = None;
    let mut out = Vec::new();
    for line in lines {
        match line {
            DiffLine::FileHeader { name, .. } => {
                highlighter = Some(new_highlighter(name));
                out.push(color::paint(enabled, color::RED, &format!("--- {name}")));
                out.push(color::paint(enabled, color::GREEN, &format!("+++ {name}")));
            }
            DiffLine::HunkMeta {
                old_start,
                old_len,
                new_start,
                new_len,
                ..
            } => out.push(color::paint(
                enabled,
                color::CYAN,
                &format!(
                    "@@ -{} +{} @@",
                    range_part(*old_start, *old_len),
                    range_part(*new_start, *new_len)
                ),
            )),
            DiffLine::Added(text) => out.push(format!(
                "{added}{}",
                render_code(text, highlighter.as_mut(), tier)
            )),
            DiffLine::Removed(text) => out.push(format!(
                "{removed}{}",
                render_code(text, highlighter.as_mut(), tier)
            )),
            DiffLine::Context(text) => out.push(format!(
                " {}",
                render_code(text, highlighter.as_mut(), tier)
            )),
        }
    }
    out.join("\n")
}

fn range_part(start: usize, len: usize) -> String {
    if len == 1 {
        format!("{start}")
    } else {
        format!("{start},{len}")
    }
}

fn render_new(lines: &[DiffLine], tier: ColorTier) -> String {
    let enabled = tier != ColorTier::Off;
    let multi = lines
        .iter()
        .filter(|line| matches!(line, DiffLine::FileHeader { .. }))
        .count()
        > 1;
    let mut highlighter: Option<HighlightLines> = None;
    let mut out = Vec::new();
    for line in lines {
        match line {
            DiffLine::FileHeader { name, .. } => {
                highlighter = Some(new_highlighter(name));
                if multi {
                    out.push(color::paint(enabled, color::DIM, name));
                }
            }
            DiffLine::Added(text) | DiffLine::Context(text) => out.push(format!(
                " {}",
                render_code(text, highlighter.as_mut(), tier)
            )),
            DiffLine::Removed(_) | DiffLine::HunkMeta { .. } => {}
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE: &str = concat!(
        "diff --git a/PKGBUILD b/PKGBUILD\nindex 1111111..2222222 100644\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
        "@@ -1,3 +1,4 @@\n line1\n-old1\n+new1\n+extra\n line3\n",
        "@@ -10 +11 @@\n line10\n-old10\n+new10\n",
        "diff --git a/foo.install b/foo.install\nindex 3333333..4444444 100644\n--- a/foo.install\n+++ b/foo.install\n",
        "@@ -1,2 +1,3 @@\n start\n+middle\n end\n",
    );

    const NEW_TWO_FILES: &str = concat!(
        "diff --git a/PKGBUILD b/PKGBUILD\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/PKGBUILD\n",
        "@@ -0,0 +1,2 @@\n+pkgname=fixture\n+pkgver=1.0.0\n",
        "diff --git a/foo.install b/foo.install\nnew file mode 100644\nindex 0000000..2222222\n--- /dev/null\n+++ b/foo.install\n",
        "@@ -0,0 +1,2 @@\n+start\n+end\n",
    );

    #[test]
    fn diff_mode_body_matches_literals_and_strips_cleanly() {
        let expected = concat!(
            "--- PKGBUILD\n",
            "+++ PKGBUILD\n",
            "@@ -1,3 +1,4 @@\n",
            " line1\n",
            "-old1\n",
            "+new1\n",
            "+extra\n",
            " line3\n",
            "@@ -10 +11 @@\n",
            " line10\n",
            "-old10\n",
            "+new10\n",
            "--- foo.install\n",
            "+++ foo.install\n",
            "@@ -1,2 +1,3 @@\n",
            " start\n",
            "+middle\n",
            " end",
        );
        let plain =
            render_package(STALE, false, ColorTier::Off).expect("stale diff has code lines");
        assert_eq!(plain, expected);
        assert!(!plain.contains('\x1b'));
        let colored_expected = concat!(
            "\x1b[1;31m--- PKGBUILD\x1b[0m\n",
            "\x1b[1;32m+++ PKGBUILD\x1b[0m\n",
            "\x1b[36m@@ -1,3 +1,4 @@\x1b[0m\n",
            " \x1b[30mline1\x1b[0m\n",
            "\x1b[1;31m-\x1b[0m\x1b[30mold1\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[30mnew1\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[30mextra\x1b[0m\n",
            " \x1b[30mline3\x1b[0m\n",
            "\x1b[36m@@ -10 +11 @@\x1b[0m\n",
            " \x1b[30mline10\x1b[0m\n",
            "\x1b[1;31m-\x1b[0m\x1b[30mold10\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[30mnew10\x1b[0m\n",
            "\x1b[1;31m--- foo.install\x1b[0m\n",
            "\x1b[1;32m+++ foo.install\x1b[0m\n",
            "\x1b[36m@@ -1,2 +1,3 @@\x1b[0m\n",
            " \x1b[30mstart\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[30mmiddle\x1b[0m\n",
            " \x1b[30mend\x1b[0m",
        );
        let colored =
            render_package(STALE, false, ColorTier::Basic16).expect("stale diff has code lines");
        assert_eq!(colored, colored_expected);
        assert_eq!(color::ansi_strip(&colored), plain);
        let truecolor =
            render_package(STALE, false, ColorTier::Truecolor).expect("stale diff has code lines");
        assert_eq!(color::ansi_strip(&truecolor), plain);
    }

    const TINY: &str = concat!(
        "diff --git a/PKGBUILD b/PKGBUILD\n--- a/PKGBUILD\n+++ b/PKGBUILD\n",
        "@@ -1,1 +1,1 @@\n-pkgname=fixture\n+pkgname=other\n",
    );

    #[test]
    fn tiered_spans_carry_syntect_colors() {
        let plain = render_package(TINY, false, ColorTier::Off).expect("tiny diff has code lines");
        let expected = concat!(
            "\x1b[1;31m--- PKGBUILD\x1b[0m\n",
            "\x1b[1;32m+++ PKGBUILD\x1b[0m\n",
            "\x1b[36m@@ -1 +1 @@\x1b[0m\n",
            "\x1b[1;31m-\x1b[0m\x1b[38;2;191;97;106mpkgname\x1b[0m\x1b[38;2;192;197;206m=\x1b[0m\x1b[38;2;163;190;140mfixture\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[38;2;191;97;106mpkgname\x1b[0m\x1b[38;2;192;197;206m=\x1b[0m\x1b[38;2;163;190;140mother\x1b[0m",
        );
        let rendered =
            render_package(TINY, false, ColorTier::Truecolor).expect("tiny diff has code lines");
        assert_eq!(rendered, expected);
        assert_eq!(color::ansi_strip(&rendered), plain);
        let expected = concat!(
            "\x1b[1;31m--- PKGBUILD\x1b[0m\n",
            "\x1b[1;32m+++ PKGBUILD\x1b[0m\n",
            "\x1b[36m@@ -1 +1 @@\x1b[0m\n",
            "\x1b[1;31m-\x1b[0m\x1b[31mpkgname\x1b[0m\x1b[90m=\x1b[0m\x1b[30mfixture\x1b[0m\n",
            "\x1b[1;32m+\x1b[0m\x1b[31mpkgname\x1b[0m\x1b[90m=\x1b[0m\x1b[30mother\x1b[0m",
        );
        let rendered =
            render_package(TINY, false, ColorTier::Basic16).expect("tiny diff has code lines");
        assert_eq!(rendered, expected);
        let code_only = rendered
            .lines()
            .skip(3)
            .collect::<Vec<_>>()
            .join("\n")
            .replace("\x1b[1;31m-\x1b[0m", "")
            .replace("\x1b[1;32m+\x1b[0m", "");
        assert!(code_only.contains("\x1b[31m"));
        assert!(code_only.contains("\x1b[90m"));
        assert!(code_only.contains("\x1b[30m"));
        assert!(!code_only.contains("\x1b[1"));
        assert_eq!(color::ansi_strip(&rendered), plain);
    }

    #[test]
    fn new_package_two_files_renders_dim_separator_between_dumps() {
        let expected = concat!(
            "PKGBUILD\n",
            " pkgname=fixture\n",
            " pkgver=1.0.0\n",
            "foo.install\n",
            " start\n",
            " end",
        );
        let plain =
            render_package(NEW_TWO_FILES, true, ColorTier::Off).expect("new files have code lines");
        assert_eq!(plain, expected);
        assert!(!plain.contains('\x1b'));
        assert!(!plain.contains("---"));
        assert!(!plain.contains("+++"));
        assert!(!plain.contains("@@"));
        let colored = render_package(NEW_TWO_FILES, true, ColorTier::Basic16)
            .expect("new files have code lines");
        assert!(colored.contains(&format!("{}foo.install{}", color::DIM, color::RESET)));
        assert_eq!(color::ansi_strip(&colored), plain);
    }

    #[test]
    fn chrome_only_diff_renders_nothing() {
        let chrome = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\nold mode 100644\nnew mode 100755\n",
            "diff --git a/data.bin b/data.bin\nBinary files a/data.bin and b/data.bin differ\n",
        );
        assert_eq!(render_package(chrome, false, ColorTier::Off), None);
        assert_eq!(render_package(chrome, false, ColorTier::Truecolor), None);
        assert_eq!(render_package(chrome, true, ColorTier::Basic16), None);
    }
}
