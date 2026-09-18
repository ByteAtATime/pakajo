use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::Context as _;

use crate::cli::prompts::confirm_review_accept;
use crate::color;
use crate::diff::{DiffLine, parse_unified_diff};
use crate::pkgbuild::{PkgbuildInfo, compute_diff};

pub fn review_pkgbuilds(pkgbuilds: &[PkgbuildInfo]) -> bool {
    let use_color = color::stdout_color();
    let mut sections: Vec<(String, String)> = Vec::new();
    for pb in pkgbuilds {
        match compute_diff(&pb.dir, pb.is_new, false) {
            Ok(raw) => {
                if let Some(body) = render_package(&raw, pb.is_new, use_color) {
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
            color::colon(use_color, "Nothing new to review - all PKGBUILDs unchanged")
        );
        return true;
    }

    let mut combined = String::new();
    for (name, body) in &sections {
        combined.push_str(&color::colon(use_color, name));
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

fn render_package(raw: &str, is_new: bool, colored: bool) -> Option<String> {
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
        return Some(render_new(&lines, colored));
    }
    Some(render_diff(&lines, colored))
}

fn render_diff(lines: &[DiffLine], colored: bool) -> String {
    let added = color::paint(colored, color::GREEN, "+");
    let removed = color::paint(colored, color::RED, "-");
    let mut out = Vec::new();
    for line in lines {
        match line {
            DiffLine::FileHeader { name, .. } => {
                out.push(color::paint(colored, color::RED, &format!("--- {name}")));
                out.push(color::paint(colored, color::GREEN, &format!("+++ {name}")));
            }
            DiffLine::HunkMeta {
                old_start,
                old_len,
                new_start,
                new_len,
                ..
            } => out.push(color::paint(
                colored,
                color::CYAN,
                &format!(
                    "@@ -{} +{} @@",
                    range_part(*old_start, *old_len),
                    range_part(*new_start, *new_len)
                ),
            )),
            DiffLine::Added(text) => out.push(format!("{added}{text}")),
            DiffLine::Removed(text) => out.push(format!("{removed}{text}")),
            DiffLine::Context(text) => out.push(format!(" {text}")),
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

fn render_new(lines: &[DiffLine], colored: bool) -> String {
    let multi = lines
        .iter()
        .filter(|line| matches!(line, DiffLine::FileHeader { .. }))
        .count()
        > 1;
    let mut out = Vec::new();
    for line in lines {
        match line {
            DiffLine::FileHeader { name, .. } => {
                if multi {
                    out.push(color::paint(colored, color::DIM, name));
                }
            }
            DiffLine::Added(text) | DiffLine::Context(text) => out.push(format!(" {text}")),
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
        let plain = render_package(STALE, false, false).expect("stale diff has code lines");
        assert_eq!(plain, expected);
        assert!(!plain.contains('\x1b'));
        let colored_expected = concat!(
            "\x1b[1;31m--- PKGBUILD\x1b[0m\n",
            "\x1b[1;32m+++ PKGBUILD\x1b[0m\n",
            "\x1b[36m@@ -1,3 +1,4 @@\x1b[0m\n",
            " line1\n",
            "\x1b[1;31m-\x1b[0mold1\n",
            "\x1b[1;32m+\x1b[0mnew1\n",
            "\x1b[1;32m+\x1b[0mextra\n",
            " line3\n",
            "\x1b[36m@@ -10 +11 @@\x1b[0m\n",
            " line10\n",
            "\x1b[1;31m-\x1b[0mold10\n",
            "\x1b[1;32m+\x1b[0mnew10\n",
            "\x1b[1;31m--- foo.install\x1b[0m\n",
            "\x1b[1;32m+++ foo.install\x1b[0m\n",
            "\x1b[36m@@ -1,2 +1,3 @@\x1b[0m\n",
            " start\n",
            "\x1b[1;32m+\x1b[0mmiddle\n",
            " end",
        );
        let colored = render_package(STALE, false, true).expect("stale diff has code lines");
        assert_eq!(colored, colored_expected);
        assert_eq!(color::ansi_strip(&colored), plain);
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
        let plain = render_package(NEW_TWO_FILES, true, false).expect("new files have code lines");
        assert_eq!(plain, expected);
        assert!(!plain.contains('\x1b'));
        assert!(!plain.contains("---"));
        assert!(!plain.contains("+++"));
        assert!(!plain.contains("@@"));
        let colored = render_package(NEW_TWO_FILES, true, true).expect("new files have code lines");
        assert!(colored.contains(&format!("{}foo.install{}", color::DIM, color::RESET)));
        assert_eq!(color::ansi_strip(&colored), plain);
    }

    #[test]
    fn chrome_only_diff_renders_nothing() {
        let chrome = concat!(
            "diff --git a/PKGBUILD b/PKGBUILD\nold mode 100644\nnew mode 100755\n",
            "diff --git a/data.bin b/data.bin\nBinary files a/data.bin and b/data.bin differ\n",
        );
        assert_eq!(render_package(chrome, false, false), None);
        assert_eq!(render_package(chrome, false, true), None);
        assert_eq!(render_package(chrome, true, false), None);
    }
}
