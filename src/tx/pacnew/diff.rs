use std::fs;
use std::path::Path;
use std::process::Command;

use crate::diff::{DiffLine, parse_unified_diff};
use crate::events::{InstallEvent, MergeHunk, MergeLine};

use super::{MergeKind, PendingMerge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HunkMeta {
    pub(super) old_start: usize,
    pub(super) old_len: usize,
    pub(super) new_start: usize,
    pub(super) new_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hunk {
    pub(super) meta: HunkMeta,
    pub(super) lines: Vec<DiffLine>,
}

fn diff_no_index(current: &Path, candidate: &Path) -> String {
    let Ok(output) = Command::new("git")
        .arg("diff")
        .arg("--no-index")
        .arg("--unified=0")
        .arg("--color=never")
        .arg(current)
        .arg(candidate)
        .output()
    else {
        return String::new();
    };
    match output.status.code() {
        Some(0) | Some(1) => {}
        _ => return String::new(),
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn group_hunks(diff_lines: &[DiffLine]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    for line in diff_lines {
        match line {
            DiffLine::HunkMeta {
                old_start,
                old_len,
                new_start,
                new_len,
                ..
            } => hunks.push(Hunk {
                meta: HunkMeta {
                    old_start: *old_start,
                    old_len: *old_len,
                    new_start: *new_start,
                    new_len: *new_len,
                },
                lines: Vec::new(),
            }),
            DiffLine::Added(_) | DiffLine::Removed(_) | DiffLine::Context(_) => {
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.push(line.clone());
                }
            }
            DiffLine::FileHeader { .. } => {}
        }
    }
    hunks
}

fn window(old_lines: &[String], first: usize, last: usize) -> Vec<String> {
    if last < first.max(1) {
        return Vec::new();
    }
    (first.max(1)..=last)
        .filter_map(|number| old_lines.get(number - 1).cloned())
        .collect()
}

fn flank(old_lines: &[String], meta: &HunkMeta) -> (Vec<String>, Vec<String>) {
    if old_lines.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if meta.old_len > 0 {
        return (
            window(
                old_lines,
                meta.old_start.saturating_sub(3),
                meta.old_start.saturating_sub(1),
            ),
            window(
                old_lines,
                meta.old_start.saturating_add(meta.old_len),
                meta.old_start
                    .saturating_add(meta.old_len)
                    .saturating_add(2),
            ),
        );
    }
    (
        window(old_lines, meta.old_start.saturating_sub(2), meta.old_start),
        window(
            old_lines,
            meta.old_start.saturating_add(1),
            meta.old_start.saturating_add(3),
        ),
    )
}

pub(super) fn candidate_path(file: &str, kind: &MergeKind) -> String {
    match kind {
        MergeKind::Pacnew => format!("{file}.pacnew"),
        MergeKind::Pacsave => format!("{file}.pacsave"),
    }
}

fn merge_line(line: &DiffLine) -> Option<MergeLine> {
    match line {
        DiffLine::Added(body) => Some(MergeLine::Added(body.clone())),
        DiffLine::Removed(body) => Some(MergeLine::Removed(body.clone())),
        _ => None,
    }
}

fn into_merge_hunk(hunk: &Hunk, before: Vec<String>, after: Vec<String>) -> MergeHunk {
    MergeHunk {
        old_start: hunk.meta.old_start,
        old_len: hunk.meta.old_len,
        new_start: hunk.meta.new_start,
        new_len: hunk.meta.new_len,
        before,
        after,
        lines: hunk.lines.iter().filter_map(merge_line).collect(),
    }
}

pub(super) fn parsed_hunks(current: &Path, candidate: &Path) -> Vec<Hunk> {
    group_hunks(&parse_unified_diff(&diff_no_index(current, candidate)))
}

fn pacnew_hunks(file: &str, candidate: &str) -> Vec<MergeHunk> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let old_lines: Vec<String> = text.lines().map(str::to_string).collect();
    parsed_hunks(Path::new(file), Path::new(candidate))
        .iter()
        .map(|hunk| {
            let (before, after) = flank(&old_lines, &hunk.meta);
            into_merge_hunk(hunk, before, after)
        })
        .collect()
}

fn pacsave_hunks(candidate: &str) -> Vec<MergeHunk> {
    parsed_hunks(Path::new("/dev/null"), Path::new(candidate))
        .iter()
        .map(|hunk| into_merge_hunk(hunk, Vec::new(), Vec::new()))
        .collect()
}

pub(super) fn offer_event(pending: &PendingMerge, index: usize, total: usize) -> InstallEvent {
    let candidate = candidate_path(&pending.file, &pending.kind);
    let hunks = match pending.kind {
        MergeKind::Pacnew => pacnew_hunks(&pending.file, &candidate),
        MergeKind::Pacsave => pacsave_hunks(&candidate),
    };
    InstallEvent::MergeOffered {
        kind: pending.kind,
        file: pending.file.clone(),
        from_noupgrade: pending.from_noupgrade,
        origin: pending.origin.clone(),
        index,
        total,
        hunks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MergeOrigin;

    fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write temp fixture");
        path
    }

    fn s(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn conf_pair(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let current = write_temp(
            dir,
            "app.conf",
            b"# confsync\nkey=local\nmid=fixed\nextra=x\n",
        );
        let candidate = write_temp(
            dir,
            "app.conf.pacnew",
            b"# confsync\nkey=new\nmid=fixed\nmode=auto\n",
        );
        (current, candidate)
    }

    fn pacnew_pending(dir: &Path) -> PendingMerge {
        PendingMerge {
            kind: MergeKind::Pacnew,
            file: dir.join("app.conf").to_string_lossy().into_owned(),
            from_noupgrade: false,
            origin: None,
        }
    }

    fn offered_hunks(pending: &PendingMerge) -> Vec<MergeHunk> {
        let InstallEvent::MergeOffered { hunks, .. } = offer_event(pending, 1, 1) else {
            unreachable!()
        };
        hunks
    }

    #[test]
    fn two_separated_changes_split_into_two_hunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (current, candidate) = conf_pair(dir.path());
        let grouped = parsed_hunks(&current, &candidate);
        assert_eq!(
            grouped.iter().map(|hunk| hunk.meta).collect::<Vec<_>>(),
            vec![
                HunkMeta {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                },
                HunkMeta {
                    old_start: 4,
                    old_len: 1,
                    new_start: 4,
                    new_len: 1,
                },
            ]
        );
    }

    #[test]
    fn identical_files_group_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let current = write_temp(dir.path(), "current.conf", b"key=same\n");
        let candidate = write_temp(dir.path(), "candidate.conf", b"key=same\n");
        assert!(parsed_hunks(&current, &candidate).is_empty());
    }

    #[test]
    fn missing_candidate_groups_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let current = write_temp(dir.path(), "current.conf", b"key=same\n");
        let missing = dir.path().join("missing.conf");
        assert!(parsed_hunks(&current, &missing).is_empty());
    }

    #[test]
    fn binary_content_groups_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let current = write_temp(dir.path(), "current.bin", b"\x00\x01\x02\x03binary\n");
        let candidate = write_temp(dir.path(), "candidate.conf", b"key=new\n");
        assert!(parsed_hunks(&current, &candidate).is_empty());
    }

    #[test]
    fn headers_dropped_and_lines_attached_to_their_hunk() {
        let parsed = vec![
            DiffLine::FileHeader {
                name: "app.conf".to_string(),
                added: 1,
                removed: 1,
            },
            DiffLine::Added("stray".to_string()),
            DiffLine::HunkMeta {
                old_start: 2,
                old_len: 1,
                new_start: 2,
                new_len: 1,
                elided: 1,
            },
            DiffLine::Removed("key=local".to_string()),
            DiffLine::Added("key=new".to_string()),
            DiffLine::HunkMeta {
                old_start: 4,
                old_len: 1,
                new_start: 4,
                new_len: 1,
                elided: 1,
            },
            DiffLine::Removed("extra=x".to_string()),
            DiffLine::Added("mode=auto".to_string()),
        ];
        let grouped = group_hunks(&parsed);
        assert_eq!(
            grouped.iter().map(|hunk| hunk.meta).collect::<Vec<_>>(),
            vec![
                HunkMeta {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                },
                HunkMeta {
                    old_start: 4,
                    old_len: 1,
                    new_start: 4,
                    new_len: 1,
                },
            ]
        );
        assert_eq!(
            grouped[0].lines,
            vec![
                DiffLine::Removed("key=local".to_string()),
                DiffLine::Added("key=new".to_string()),
            ]
        );
        assert_eq!(
            grouped[1].lines,
            vec![
                DiffLine::Removed("extra=x".to_string()),
                DiffLine::Added("mode=auto".to_string()),
            ]
        );
    }

    #[test]
    fn mid_file_hunk_flanks_three_lines_each_side() {
        assert_eq!(
            flank(
                &s(&[
                    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"
                ]),
                &HunkMeta {
                    old_start: 5,
                    old_len: 2,
                    new_start: 90,
                    new_len: 9,
                },
            ),
            (s(&["two", "three", "four"]), s(&["seven", "eight", "nine"]),)
        );
    }

    #[test]
    fn hunk_at_file_start_has_empty_before() {
        assert_eq!(
            flank(
                &s(&[
                    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"
                ]),
                &HunkMeta {
                    old_start: 1,
                    old_len: 1,
                    new_start: 90,
                    new_len: 9,
                },
            ),
            (s(&[]), s(&["two", "three", "four"])),
        );
    }

    #[test]
    fn hunk_at_file_end_clamps_after() {
        assert_eq!(
            flank(
                &s(&["one", "two", "three", "four"]),
                &HunkMeta {
                    old_start: 4,
                    old_len: 1,
                    new_start: 90,
                    new_len: 9,
                },
            ),
            (s(&["one", "two", "three"]), s(&[])),
        );
    }

    #[test]
    fn insertion_hunk_mid_file_flanks_its_position() {
        assert_eq!(
            flank(
                &s(&["one", "two", "three", "four", "five", "six"]),
                &HunkMeta {
                    old_start: 3,
                    old_len: 0,
                    new_start: 90,
                    new_len: 9,
                },
            ),
            (s(&["one", "two", "three"]), s(&["four", "five", "six"]),)
        );
    }

    #[test]
    fn insertion_at_line_zero_has_empty_before() {
        assert_eq!(
            flank(
                &s(&["one", "two", "three", "four"]),
                &HunkMeta {
                    old_start: 0,
                    old_len: 0,
                    new_start: 90,
                    new_len: 9,
                },
            ),
            (s(&[]), s(&["one", "two", "three"])),
        );
    }

    #[test]
    fn pacnew_offer_carries_flanked_hunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = pacnew_pending(dir.path());
        conf_pair(dir.path());
        assert_eq!(
            offered_hunks(&pending),
            vec![
                MergeHunk {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                    before: vec!["# confsync".to_string()],
                    after: vec!["mid=fixed".to_string(), "extra=x".to_string()],
                    lines: vec![
                        MergeLine::Removed("key=local".to_string()),
                        MergeLine::Added("key=new".to_string()),
                    ],
                },
                MergeHunk {
                    old_start: 4,
                    old_len: 1,
                    new_start: 4,
                    new_len: 1,
                    before: vec![
                        "# confsync".to_string(),
                        "key=local".to_string(),
                        "mid=fixed".to_string(),
                    ],
                    after: Vec::new(),
                    lines: vec![
                        MergeLine::Removed("extra=x".to_string()),
                        MergeLine::Added("mode=auto".to_string()),
                    ],
                },
            ]
        );
    }

    #[test]
    fn pacsave_offer_carries_empty_flanks_with_added_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("app.conf").to_string_lossy().into_owned();
        write_temp(dir.path(), "app.conf.pacsave", b"key=local\n");
        let pending = PendingMerge {
            kind: MergeKind::Pacsave,
            file,
            from_noupgrade: false,
            origin: None,
        };
        assert_eq!(
            offered_hunks(&pending),
            vec![MergeHunk {
                old_start: 0,
                old_len: 0,
                new_start: 1,
                new_len: 1,
                before: Vec::new(),
                after: Vec::new(),
                lines: vec![MergeLine::Added("key=local".to_string())],
            }]
        );
    }

    #[test]
    fn offer_event_passes_through_identity_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut pending = pacnew_pending(dir.path());
        pending.from_noupgrade = true;
        pending.origin = Some(MergeOrigin {
            package: "pacman".to_string(),
            old_version: Some("6.0-1".to_string()),
            new_version: Some("7.0-1".to_string()),
        });
        let InstallEvent::MergeOffered {
            kind,
            file,
            from_noupgrade,
            origin,
            index,
            total,
            ..
        } = offer_event(&pending, 2, 3)
        else {
            unreachable!()
        };
        assert_eq!(kind, MergeKind::Pacnew);
        assert_eq!(file, pending.file);
        assert!(from_noupgrade);
        assert_eq!(origin, pending.origin);
        assert_eq!((index, total), (2, 3));
    }

    #[test]
    fn missing_current_file_offers_empty_hunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = pacnew_pending(dir.path());
        assert!(offered_hunks(&pending).is_empty());
    }
}
