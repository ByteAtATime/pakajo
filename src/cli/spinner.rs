use std::io::IsTerminal as _;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::color;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: Duration = Duration::from_millis(80);

pub fn frame_text(frame: usize, message: &str, elapsed: Duration, colored: bool) -> String {
    let glyph = color::paint(colored, color::CYAN, FRAMES[frame % FRAMES.len()]);
    format!("{glyph} {message} ({}s)", elapsed.as_secs())
}

pub struct Spinner {
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(message: &str) -> Self {
        if !std::io::stderr().is_terminal() {
            eprintln!("{message}...");
            return Self {
                running: Arc::new(AtomicBool::new(false)),
                handle: None,
            };
        }
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let msg = message.to_string();
        let handle = std::thread::spawn(move || {
            let began = Instant::now();
            let mut frame = 0usize;
            let mut err = std::io::stderr();
            loop {
                if !flag.load(Ordering::Relaxed) {
                    break;
                }
                let text = frame_text(frame, &msg, began.elapsed(), true);
                let _ = write!(err, "\r\x1b[2K{text}");
                let _ = err.flush();
                frame += 1;
                std::thread::sleep(TICK);
            }
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        });
        Self {
            running,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn frame_text_cycles_braille_frames() {
        let zero = frame_text(0, "working", Duration::ZERO, false);
        let ten = frame_text(FRAMES.len(), "working", Duration::ZERO, false);
        assert_eq!(zero, ten, "frames must wrap modulo FRAMES.len()");
    }

    #[test]
    fn frame_text_contains_message_and_elapsed_seconds() {
        let text = frame_text(3, "refreshing package index", Duration::from_secs(7), false);
        assert!(text.contains("refreshing package index"));
        assert!(text.contains("(7s)"));
    }

    #[test]
    fn frame_text_escapes_only_when_colored() {
        let plain = frame_text(0, "m", Duration::ZERO, false);
        let colored = frame_text(0, "m", Duration::ZERO, true);
        assert!(!plain.contains('\u{1b}'));
        assert!(colored.contains('\u{1b}'));
    }
}
