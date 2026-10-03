use std::cell::RefCell;
use std::io::BufRead;
use std::rc::Rc;

use crate::events::{InstallEvent, InstallSink, LogLevel};

use super::apply::{ApplyError, apply_decision};
use super::diff::offer_event;
use super::{MergeAction, MergeKind, PendingMerge, parse_decision_line};

pub struct MergeRecorder {
    inner: Box<dyn InstallSink>,
    pending: Rc<RefCell<Vec<PendingMerge>>>,
}

impl MergeRecorder {
    pub fn new(inner: Box<dyn InstallSink>, pending: Rc<RefCell<Vec<PendingMerge>>>) -> Self {
        Self { inner, pending }
    }
}

fn record_pending(event: &InstallEvent) -> Option<PendingMerge> {
    match event {
        InstallEvent::PacnewCreated {
            from_noupgrade,
            file,
            origin,
        } => Some(PendingMerge {
            kind: MergeKind::Pacnew,
            file: file.clone(),
            from_noupgrade: *from_noupgrade,
            origin: origin.clone(),
        }),
        InstallEvent::PacsaveCreated { file, origin } => Some(PendingMerge {
            kind: MergeKind::Pacsave,
            file: file.clone(),
            from_noupgrade: false,
            origin: origin.clone(),
        }),
        _ => None,
    }
}

impl InstallSink for MergeRecorder {
    fn event(&mut self, event: InstallEvent) {
        if let Some(pending) = record_pending(&event) {
            self.pending.borrow_mut().push(pending);
        }
        self.inner.event(event);
    }
}

fn apply_reason(error: &ApplyError) -> String {
    match error {
        ApplyError::Io(source) => format!("io error: {source}"),
        ApplyError::Unsupported(payload) => format!("unsupported action {payload}"),
    }
}

fn read_merge_line<R: BufRead>(input: &Rc<RefCell<R>>) -> String {
    let mut line = String::new();
    match input.borrow_mut().read_line(&mut line) {
        Ok(0) | Err(_) => String::new(),
        Ok(_) => line,
    }
}

fn decide_action(line: &str, file: &str) -> MergeAction {
    if line.trim().is_empty() {
        return MergeAction::Defer;
    }
    parse_decision_line(line, file)
}

fn resolved_event(pending: &PendingMerge, action: MergeAction) -> InstallEvent {
    InstallEvent::MergeResolved {
        kind: pending.kind,
        file: pending.file.clone(),
        action,
    }
}

fn offer_hunks(event: &InstallEvent) -> bool {
    match event {
        InstallEvent::MergeOffered { hunks, .. } => !hunks.is_empty(),
        _ => false,
    }
}

fn resolve_pending<R: BufRead, S: InstallSink + ?Sized>(
    sink: &mut S,
    input: &Rc<RefCell<R>>,
    pending: &PendingMerge,
    index: usize,
    total: usize,
) {
    let offer = offer_event(pending, index, total);
    if !offer_hunks(&offer) {
        sink.event(resolved_event(pending, MergeAction::Defer));
        return;
    }
    sink.event(offer);
    let action = decide_action(&read_merge_line(input), &pending.file);
    match apply_decision(pending, &action) {
        Ok(performed) => sink.event(resolved_event(pending, performed)),
        Err(error) => {
            sink.event(InstallEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "merge decision for {} deferred: {}",
                    pending.file,
                    apply_reason(&error)
                ),
            });
            sink.event(resolved_event(pending, MergeAction::Defer));
        }
    }
}

pub fn run_merge_phase<R: BufRead, S: InstallSink + ?Sized>(
    sink: &mut S,
    input: &Rc<RefCell<R>>,
    pending: &Rc<RefCell<Vec<PendingMerge>>>,
) {
    let offers = std::mem::take(&mut *pending.borrow_mut());
    if offers.is_empty() {
        return;
    }
    let total = offers.len();
    for (position, offer) in offers.iter().enumerate() {
        resolve_pending(sink, input, offer, position + 1, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MergeOrigin;
    use std::io::Cursor;

    struct VecSink {
        seen: Rc<RefCell<Vec<InstallEvent>>>,
    }

    impl InstallSink for VecSink {
        fn event(&mut self, event: InstallEvent) {
            self.seen.borrow_mut().push(event);
        }
    }

    fn sink() -> (VecSink, Rc<RefCell<Vec<InstallEvent>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        (
            VecSink {
                seen: Rc::clone(&seen),
            },
            seen,
        )
    }

    fn pending(dir: &std::path::Path, kind: MergeKind) -> PendingMerge {
        PendingMerge {
            kind,
            file: dir.join("app.conf").to_string_lossy().into_owned(),
            from_noupgrade: false,
            origin: None,
        }
    }

    fn input(data: &[u8]) -> Rc<RefCell<Cursor<Vec<u8>>>> {
        Rc::new(RefCell::new(Cursor::new(data.to_vec())))
    }

    fn resolved_at(events: &[InstallEvent], index: usize) -> &InstallEvent {
        events.get(index).expect("event present")
    }

    fn is_defer_for(events: &[InstallEvent], index: usize, file: &str) -> bool {
        matches!(
            resolved_at(events, index),
            InstallEvent::MergeResolved {
                action: MergeAction::Defer,
                file: actual,
                ..
            } if actual == file
        )
    }

    #[test]
    fn honored_take_new_decision_renames_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let line = format!("{{\"file\":\"{file}\",\"action\":\"TakeNew\"}}\n");
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(line.as_bytes()),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 2);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(matches!(
            &viewed[1],
            InstallEvent::MergeResolved {
                action: MergeAction::TakeNew,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.conf")).expect("read file"),
            "key=new\n"
        );
    }

    #[test]
    fn garbage_line_resolves_to_defer() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(b"not json at all\n"),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 2);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(is_defer_for(&viewed, 1, &file));
    }

    #[test]
    fn empty_line_emits_offer_then_defers() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(b"\n"),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 2);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(is_defer_for(&viewed, 1, &file));
    }

    #[test]
    fn eof_emits_offer_then_defers() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(b""),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 2);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(is_defer_for(&viewed, 1, &file));
    }

    #[test]
    fn apply_failure_warns_before_resolving_defer() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=live\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacsave"), b"key=saved\n").expect("write backup");
        let offer = pending(dir.path(), MergeKind::Pacsave);
        let file = offer.file.clone();
        let line = format!("{{\"file\":\"{file}\",\"action\":\"TakeNew\"}}\n");
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(line.as_bytes()),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 3);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(matches!(
            &viewed[1],
            InstallEvent::Log {
                level: LogLevel::Warning,
                message,
            } if message == &format!("merge decision for {file} deferred: unsupported action TakeNew")
        ));
        assert!(is_defer_for(&viewed, 2, &file));
    }

    #[test]
    fn io_failure_warns_before_resolving_defer() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=local\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=new\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let line = format!("{{\"file\":\"{file}\",\"action\":\"TakeNew\"}}\n");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555))
            .expect("lock dir");
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(line.as_bytes()),
            &Rc::new(RefCell::new(vec![offer])),
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("unlock dir");
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 3);
        assert!(matches!(&viewed[0], InstallEvent::MergeOffered { .. }));
        assert!(matches!(
            &viewed[1],
            InstallEvent::Log {
                level: LogLevel::Warning,
                message,
            } if message == &format!("merge decision for {file} deferred: io error: Permission denied (os error 13)")
        ));
        assert!(is_defer_for(&viewed, 2, &file));
    }

    #[test]
    fn identical_files_resolve_without_offer() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=same\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacnew"), b"key=same\n").expect("write candidate");
        let offer = pending(dir.path(), MergeKind::Pacnew);
        let file = offer.file.clone();
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(b""),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 1);
        assert!(is_defer_for(&viewed, 0, &file));
    }

    #[test]
    fn resolutions_carry_kind_and_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("app.conf"), b"key=live\n").expect("write current");
        std::fs::write(dir.path().join("app.conf.pacsave"), b"key=saved\n").expect("write backup");
        let offer = pending(dir.path(), MergeKind::Pacsave);
        let line = format!("{{\"file\":\"{}\",\"action\":\"Delete\"}}\n", offer.file);
        let (mut events, seen) = sink();
        run_merge_phase(
            &mut events,
            &input(line.as_bytes()),
            &Rc::new(RefCell::new(vec![offer])),
        );
        let viewed = seen.borrow();
        assert_eq!(viewed.len(), 2);
        match &viewed[1] {
            InstallEvent::MergeResolved { kind, file, action } => {
                assert_eq!(*kind, MergeKind::Pacsave);
                assert_eq!(*action, MergeAction::Delete);
                assert!(file.ends_with("app.conf"));
            }
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn recorder_collects_pacnew_with_origin_passthrough() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let shared = Rc::new(RefCell::new(Vec::new()));
        let origin = Some(MergeOrigin {
            package: "pacman".to_string(),
            old_version: Some("6.0-1".to_string()),
            new_version: Some("7.0-1".to_string()),
        });
        let mut recorder = MergeRecorder::new(
            Box::new(VecSink {
                seen: Rc::clone(&seen),
            }),
            Rc::clone(&shared),
        );
        recorder.event(InstallEvent::PacnewCreated {
            from_noupgrade: true,
            file: "/etc/a.conf".to_string(),
            origin: origin.clone(),
        });
        recorder.event(InstallEvent::PacsaveCreated {
            file: "/etc/b.conf".to_string(),
            origin: origin.clone(),
        });
        assert_eq!(
            *shared.borrow(),
            vec![
                PendingMerge {
                    kind: MergeKind::Pacnew,
                    file: "/etc/a.conf".to_string(),
                    from_noupgrade: true,
                    origin: origin.clone(),
                },
                PendingMerge {
                    kind: MergeKind::Pacsave,
                    file: "/etc/b.conf".to_string(),
                    from_noupgrade: false,
                    origin,
                },
            ]
        );
        assert_eq!(seen.borrow().len(), 2);
    }

    #[test]
    fn recorder_forwards_unrelated_events_unchanged() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let shared = Rc::new(RefCell::new(Vec::new()));
        let mut recorder = MergeRecorder::new(
            Box::new(VecSink {
                seen: Rc::clone(&seen),
            }),
            Rc::clone(&shared),
        );
        recorder.event(InstallEvent::TransactionDone);
        assert!(shared.borrow().is_empty());
        assert_eq!(seen.borrow().len(), 1);
        assert!(matches!(seen.borrow()[0], InstallEvent::TransactionDone));
    }
}
