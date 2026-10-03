use std::fs;
use std::path::Path;

use crate::diff::DiffLine;

use super::diff::{Hunk, candidate_path, parsed_hunks};
use super::{MergeAction, MergeKind, PendingMerge};

#[derive(Debug)]
pub(super) enum ApplyError {
    Unsupported(String),
    Io(std::io::Error),
}

fn unsupported(action: &MergeAction) -> ApplyError {
    ApplyError::Unsupported(format!("{action:?}"))
}

fn old_side(lines: &[DiffLine]) -> Vec<&str> {
    lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::Removed(body) | DiffLine::Context(body) => Some(body.as_str()),
            DiffLine::Added(_) | DiffLine::FileHeader { .. } | DiffLine::HunkMeta { .. } => None,
        })
        .collect()
}

fn new_side(lines: &[DiffLine]) -> Vec<&str> {
    lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::Added(body) | DiffLine::Context(body) => Some(body.as_str()),
            DiffLine::Removed(_) | DiffLine::FileHeader { .. } | DiffLine::HunkMeta { .. } => None,
        })
        .collect()
}

fn orig_lines(orig: &str) -> (Vec<&str>, bool) {
    let trailing = orig.ends_with('\n');
    let body = orig.strip_suffix('\n').unwrap_or(orig);
    if body.is_empty() {
        return (Vec::new(), trailing);
    }
    (body.split('\n').collect(), trailing)
}

fn copy_through(output: &mut Vec<String>, source: &[&str], cursor: &mut usize, end: usize) {
    while *cursor < end {
        if let Some(line) = source.get(*cursor - 1) {
            output.push((*line).to_string());
        }
        *cursor += 1;
    }
}

fn emit_hunk(
    output: &mut Vec<String>,
    source: &[&str],
    cursor: &mut usize,
    hunk: &Hunk,
    selected: bool,
) -> Result<(), ApplyError> {
    let start = hunk.meta.old_start;
    let len = hunk.meta.old_len;
    if len == 0 {
        if start + 1 < *cursor {
            return Err(ApplyError::Unsupported(format!("out of order {start}")));
        }
        copy_through(output, source, cursor, start + 1);
        if selected {
            output.extend(new_side(&hunk.lines).iter().map(|line| (*line).to_string()));
        }
        return Ok(());
    }
    if start < *cursor {
        return Err(ApplyError::Unsupported(format!("out of order {start}")));
    }
    copy_through(output, source, cursor, start);
    if selected {
        output.extend(new_side(&hunk.lines).iter().map(|line| (*line).to_string()));
    } else {
        output.extend(old_side(&hunk.lines).iter().map(|line| (*line).to_string()));
    }
    *cursor = start + len;
    Ok(())
}

pub(super) fn apply_selected_hunks(
    orig: &str,
    hunks: &[Hunk],
    selected: &[usize],
) -> Result<String, ApplyError> {
    if selected.iter().any(|index| *index >= hunks.len()) {
        return Err(ApplyError::Unsupported(format!("hunk {selected:?}")));
    }
    let (source, trailing) = orig_lines(orig);
    let mut output: Vec<String> = Vec::new();
    let mut cursor = 1;
    for (index, hunk) in hunks.iter().enumerate() {
        emit_hunk(
            &mut output,
            &source,
            &mut cursor,
            hunk,
            selected.contains(&index),
        )?;
    }
    copy_through(&mut output, &source, &mut cursor, source.len() + 1);
    if output.is_empty() {
        return Ok(String::new());
    }
    let mut merged = output.join("\n");
    if trailing {
        merged.push('\n');
    }
    Ok(merged)
}

fn merge_pacnew(pending: &PendingMerge, hunks: &[usize]) -> Result<MergeAction, ApplyError> {
    let candidate = candidate_path(&pending.file, &pending.kind);
    let grouped = parsed_hunks(Path::new(&pending.file), Path::new(&candidate));
    if grouped.is_empty() {
        return Err(unsupported(&MergeAction::Merge {
            hunks: hunks.to_vec(),
        }));
    }
    let current = fs::read_to_string(&pending.file).map_err(ApplyError::Io)?;
    let merged = apply_selected_hunks(&current, &grouped, hunks)?;
    fs::write(&pending.file, merged).map_err(ApplyError::Io)?;
    fs::remove_file(&candidate).map_err(ApplyError::Io)?;
    Ok(MergeAction::Merge {
        hunks: hunks.to_vec(),
    })
}

pub(super) fn apply_decision(
    pending: &PendingMerge,
    action: &MergeAction,
) -> Result<MergeAction, ApplyError> {
    if matches!(action, MergeAction::Defer) {
        return Ok(MergeAction::Defer);
    }
    match (&pending.kind, action) {
        (MergeKind::Pacnew, MergeAction::KeepCurrent) => {
            match fs::remove_file(candidate_path(&pending.file, &pending.kind)) {
                Ok(()) => Ok(MergeAction::KeepCurrent),
                Err(source) => Err(ApplyError::Io(source)),
            }
        }
        (MergeKind::Pacnew, MergeAction::TakeNew) => {
            match fs::rename(candidate_path(&pending.file, &pending.kind), &pending.file) {
                Ok(()) => Ok(MergeAction::TakeNew),
                Err(source) => Err(ApplyError::Io(source)),
            }
        }
        (MergeKind::Pacsave, MergeAction::Restore) => {
            match fs::rename(candidate_path(&pending.file, &pending.kind), &pending.file) {
                Ok(()) => Ok(MergeAction::Restore),
                Err(source) => Err(ApplyError::Io(source)),
            }
        }
        (MergeKind::Pacsave, MergeAction::Delete) => {
            match fs::remove_file(candidate_path(&pending.file, &pending.kind)) {
                Ok(()) => Ok(MergeAction::Delete),
                Err(source) => Err(ApplyError::Io(source)),
            }
        }
        (MergeKind::Pacnew, MergeAction::Merge { hunks }) => merge_pacnew(pending, hunks),
        _ => Err(unsupported(action)),
    }
}

pub(super) fn merge_disposition(
    kind: MergeKind,
    action: &MergeAction,
    file: &str,
) -> Option<String> {
    match (kind, action) {
        (MergeKind::Pacnew, MergeAction::KeepCurrent) => {
            Some(format!("kept current, removed {file}.pacnew"))
        }
        (MergeKind::Pacnew, MergeAction::TakeNew) => {
            Some(format!("installed new, removed {file}.pacnew"))
        }
        (MergeKind::Pacnew, MergeAction::Merge { .. }) => {
            Some(format!("merged, removed {file}.pacnew"))
        }
        (MergeKind::Pacnew, MergeAction::Defer) => Some(format!("kept {file}.pacnew for later")),
        (MergeKind::Pacsave, MergeAction::Restore) => Some(format!("restored {file}.pacsave")),
        (MergeKind::Pacsave, MergeAction::Delete) => Some(format!("removed {file}.pacsave")),
        (MergeKind::Pacsave, MergeAction::Defer) => Some(format!("kept {file}.pacsave for later")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::diff::HunkMeta;
    use super::*;

    fn pending(dir: &Path, kind: MergeKind) -> PendingMerge {
        PendingMerge {
            kind,
            file: dir.join("app.conf").to_string_lossy().into_owned(),
            from_noupgrade: false,
            origin: None,
        }
    }

    fn conf_fixture(dir: &Path) -> PendingMerge {
        fs::write(dir.join("app.conf"), b"key=local\nmid=fixed\n").expect("write current");
        fs::write(dir.join("app.conf.pacnew"), b"key=new\nmid=fixed\n").expect("write candidate");
        pending(dir, MergeKind::Pacnew)
    }

    fn save_fixture(dir: &Path) -> PendingMerge {
        fs::write(dir.join("app.conf"), b"key=live\n").expect("write current");
        fs::write(dir.join("app.conf.pacsave"), b"key=saved\n").expect("write candidate");
        pending(dir, MergeKind::Pacsave)
    }

    #[test]
    fn selected_insertion_hunk_splices_new_line_between_old_lines() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 1,
                old_len: 0,
                new_start: 2,
                new_len: 1,
            },
            lines: vec![DiffLine::Added("b".to_string())],
        }];
        assert_eq!(
            apply_selected_hunks("a\nc\n", &hunks, &[0]).expect("merge"),
            "a\nb\nc\n"
        );
    }

    #[test]
    fn selected_removal_hunk_drops_old_line() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 2,
                old_len: 1,
                new_start: 2,
                new_len: 0,
            },
            lines: vec![DiffLine::Removed("b".to_string())],
        }];
        assert_eq!(
            apply_selected_hunks("a\nb\n", &hunks, &[0]).expect("merge"),
            "a\n"
        );
    }

    #[test]
    fn unselected_hunk_keeps_old_lines_while_selected_hunk_applies() {
        let hunks = vec![
            Hunk {
                meta: HunkMeta {
                    old_start: 1,
                    old_len: 1,
                    new_start: 1,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("a".to_string()),
                    DiffLine::Added("x".to_string()),
                ],
            },
            Hunk {
                meta: HunkMeta {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("b".to_string()),
                    DiffLine::Added("y".to_string()),
                ],
            },
        ];
        assert_eq!(
            apply_selected_hunks("a\nb\n", &hunks, &[0]).expect("merge"),
            "x\nb\n"
        );
    }

    #[test]
    fn changed_final_line_without_trailing_newline_stays_unterminated() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 2,
                old_len: 1,
                new_start: 2,
                new_len: 1,
            },
            lines: vec![
                DiffLine::Removed("b".to_string()),
                DiffLine::Added("x".to_string()),
            ],
        }];
        assert_eq!(
            apply_selected_hunks("a\nb", &hunks, &[0]).expect("merge"),
            "a\nx"
        );
    }

    #[test]
    fn untouched_final_line_without_trailing_newline_stays_unterminated() {
        let hunks = vec![
            Hunk {
                meta: HunkMeta {
                    old_start: 1,
                    old_len: 1,
                    new_start: 1,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("a".to_string()),
                    DiffLine::Added("x".to_string()),
                ],
            },
            Hunk {
                meta: HunkMeta {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("b".to_string()),
                    DiffLine::Added("y".to_string()),
                ],
            },
        ];
        assert_eq!(
            apply_selected_hunks("a\nb", &hunks, &[0]).expect("merge"),
            "x\nb"
        );
    }

    #[test]
    fn merged_result_keeps_trailing_newline_of_orig() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 2,
                old_len: 1,
                new_start: 2,
                new_len: 1,
            },
            lines: vec![
                DiffLine::Removed("b".to_string()),
                DiffLine::Added("x".to_string()),
            ],
        }];
        assert_eq!(
            apply_selected_hunks("a\nb\n", &hunks, &[0]).expect("merge"),
            "a\nx\n"
        );
    }

    #[test]
    fn selecting_removal_of_the_only_line_empties_output() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 0,
            },
            lines: vec![DiffLine::Removed("a".to_string())],
        }];
        assert_eq!(
            apply_selected_hunks("a\n", &hunks, &[0]).expect("merge"),
            String::new()
        );
    }

    #[test]
    fn out_of_order_selection_applies_both_hunks() {
        let hunks = vec![
            Hunk {
                meta: HunkMeta {
                    old_start: 1,
                    old_len: 1,
                    new_start: 1,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("a".to_string()),
                    DiffLine::Added("x".to_string()),
                ],
            },
            Hunk {
                meta: HunkMeta {
                    old_start: 2,
                    old_len: 1,
                    new_start: 2,
                    new_len: 1,
                },
                lines: vec![
                    DiffLine::Removed("b".to_string()),
                    DiffLine::Added("y".to_string()),
                ],
            },
        ];
        assert_eq!(
            apply_selected_hunks("a\nb\n", &hunks, &[1, 0]).expect("merge"),
            "x\ny\n"
        );
    }

    #[test]
    fn duplicate_selection_applies_once() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
            },
            lines: vec![
                DiffLine::Removed("a".to_string()),
                DiffLine::Added("x".to_string()),
            ],
        }];
        assert_eq!(
            apply_selected_hunks("a\n", &hunks, &[0, 0]).expect("merge"),
            "x\n"
        );
    }

    #[test]
    fn out_of_range_selection_is_unsupported() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
            },
            lines: vec![
                DiffLine::Removed("a".to_string()),
                DiffLine::Added("x".to_string()),
            ],
        }];
        assert!(matches!(
            apply_selected_hunks("a\n", &hunks, &[2]),
            Err(ApplyError::Unsupported(_))
        ));
    }

    #[test]
    fn empty_selection_returns_original() {
        let hunks = vec![Hunk {
            meta: HunkMeta {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
            },
            lines: vec![
                DiffLine::Removed("a".to_string()),
                DiffLine::Added("x".to_string()),
            ],
        }];
        assert_eq!(
            apply_selected_hunks("a\n", &hunks, &[]).expect("merge"),
            "a\n"
        );
    }

    #[test]
    fn take_new_renames_candidate_onto_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = conf_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::TakeNew).expect("take new"),
            MergeAction::TakeNew
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=new\nmid=fixed\n"
        );
        assert!(!dir.path().join("app.conf.pacnew").exists());
    }

    #[test]
    fn keep_current_removes_candidate_and_keeps_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = conf_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::KeepCurrent).expect("keep current"),
            MergeAction::KeepCurrent
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=local\nmid=fixed\n"
        );
        assert!(!dir.path().join("app.conf.pacnew").exists());
    }

    #[test]
    fn restore_renames_pacsave_onto_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = save_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::Restore).expect("restore"),
            MergeAction::Restore
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=saved\n"
        );
        assert!(!dir.path().join("app.conf.pacsave").exists());
    }

    #[test]
    fn delete_removes_pacsave_and_keeps_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = save_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::Delete).expect("delete"),
            MergeAction::Delete
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=live\n"
        );
        assert!(!dir.path().join("app.conf.pacsave").exists());
    }

    #[test]
    fn merge_writes_merged_content_and_removes_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = conf_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::Merge { hunks: vec![0] }).expect("merge"),
            MergeAction::Merge { hunks: vec![0] }
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=new\nmid=fixed\n"
        );
        assert!(!dir.path().join("app.conf.pacnew").exists());
    }

    #[test]
    fn defer_touches_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = conf_fixture(dir.path());
        assert_eq!(
            apply_decision(&pending, &MergeAction::Defer).expect("defer"),
            MergeAction::Defer
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=local\nmid=fixed\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf.pacnew")).expect("read candidate"),
            "key=new\nmid=fixed\n"
        );
    }

    #[test]
    fn mismatched_kind_and_action_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = save_fixture(dir.path());
        assert!(matches!(
            apply_decision(&pending, &MergeAction::KeepCurrent),
            Err(ApplyError::Unsupported(_))
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=live\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf.pacsave")).expect("read candidate"),
            "key=saved\n"
        );
    }

    #[test]
    fn merge_with_missing_current_file_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let pending = pending(dir.path(), MergeKind::Pacnew);
        assert!(apply_decision(&pending, &MergeAction::Merge { hunks: vec![0] }).is_err());
        assert_eq!(
            fs::read(dir.path().join("app.conf.pacnew")).expect("read candidate"),
            b"key=new\n"
        );
        assert!(!dir.path().join("app.conf").exists());
    }

    #[test]
    fn delete_leaves_rotated_sibling_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pending = save_fixture(dir.path());
        fs::write(dir.path().join("app.conf.pacsave.1"), b"key=older\n").expect("write rotated");
        assert_eq!(
            apply_decision(&pending, &MergeAction::Delete).expect("delete"),
            MergeAction::Delete
        );
        assert!(!dir.path().join("app.conf.pacsave").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf.pacsave.1")).expect("read rotated"),
            "key=older\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=live\n"
        );
    }

    #[test]
    fn take_new_with_missing_candidate_reports_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        let pending = pending(dir.path(), MergeKind::Pacnew);
        assert!(matches!(
            apply_decision(&pending, &MergeAction::TakeNew),
            Err(ApplyError::Io(ref source))
                if source.kind() == std::io::ErrorKind::NotFound
        ));
        assert_eq!(
            fs::read(dir.path().join("app.conf")).expect("read file"),
            b"key=local\n"
        );
    }

    #[test]
    fn keep_current_disposition_names_the_removed_pacnew() {
        assert_eq!(
            merge_disposition(
                MergeKind::Pacnew,
                &MergeAction::KeepCurrent,
                "/etc/app.conf"
            ),
            Some("kept current, removed /etc/app.conf.pacnew".to_string())
        );
    }

    #[test]
    fn take_new_disposition_names_the_removed_pacnew() {
        assert_eq!(
            merge_disposition(MergeKind::Pacnew, &MergeAction::TakeNew, "/etc/app.conf"),
            Some("installed new, removed /etc/app.conf.pacnew".to_string())
        );
    }

    #[test]
    fn merge_disposition_names_the_removed_pacnew() {
        assert_eq!(
            merge_disposition(
                MergeKind::Pacnew,
                &MergeAction::Merge { hunks: vec![0] },
                "/etc/app.conf"
            ),
            Some("merged, removed /etc/app.conf.pacnew".to_string())
        );
    }

    #[test]
    fn deferred_pacnew_disposition_names_the_kept_pacnew() {
        assert_eq!(
            merge_disposition(MergeKind::Pacnew, &MergeAction::Defer, "/etc/app.conf"),
            Some("kept /etc/app.conf.pacnew for later".to_string())
        );
    }

    #[test]
    fn restore_disposition_names_the_restored_pacsave() {
        assert_eq!(
            merge_disposition(MergeKind::Pacsave, &MergeAction::Restore, "/etc/app.conf"),
            Some("restored /etc/app.conf.pacsave".to_string())
        );
    }

    #[test]
    fn delete_disposition_names_the_removed_pacsave() {
        assert_eq!(
            merge_disposition(MergeKind::Pacsave, &MergeAction::Delete, "/etc/app.conf"),
            Some("removed /etc/app.conf.pacsave".to_string())
        );
    }

    #[test]
    fn deferred_pacsave_disposition_names_the_kept_pacsave() {
        assert_eq!(
            merge_disposition(MergeKind::Pacsave, &MergeAction::Defer, "/etc/app.conf"),
            Some("kept /etc/app.conf.pacsave for later".to_string())
        );
    }

    #[test]
    fn pacsave_with_take_new_has_no_disposition() {
        assert_eq!(
            merge_disposition(MergeKind::Pacsave, &MergeAction::TakeNew, "/etc/app.conf"),
            None
        );
    }

    #[test]
    fn pacnew_with_restore_has_no_disposition() {
        assert_eq!(
            merge_disposition(MergeKind::Pacnew, &MergeAction::Restore, "/etc/app.conf"),
            None
        );
    }

    #[test]
    fn unsupported_payload_names_action() {
        match unsupported(&MergeAction::Defer) {
            ApplyError::Unsupported(payload) => assert_eq!(payload, "Defer"),
            ApplyError::Io(_) => panic!("expected unsupported"),
        }
    }
}
