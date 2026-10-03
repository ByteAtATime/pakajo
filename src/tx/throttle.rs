use std::collections::HashMap;

use crate::events::InstallEvent;

const WINDOW_MS: u64 = 100;

#[derive(Debug, Clone, Copy)]
struct EmittedProgress {
    downloaded: i64,
    at_ms: u64,
}

#[derive(Debug, Default)]
pub struct DownloadThrottle {
    last_emitted: HashMap<String, EmittedProgress>,
}

impl DownloadThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn admit(&mut self, event: &InstallEvent, now_ms: u64) -> bool {
        match event {
            InstallEvent::DownloadProgress {
                filename,
                downloaded,
                total,
            } => self.admit_progress(filename, *downloaded, *total, now_ms),
            InstallEvent::DownloadInit { filename, .. } => {
                self.last_emitted.remove(filename);
                true
            }
            InstallEvent::DownloadCompleted { filename, .. } => {
                self.last_emitted.remove(filename);
                true
            }
            InstallEvent::DownloadRetry { .. } => true,
            _ => true,
        }
    }

    fn admit_progress(&mut self, filename: &str, downloaded: i64, total: i64, now_ms: u64) -> bool {
        if let Some(previous) = self.last_emitted.get(filename) {
            if downloaded == previous.downloaded {
                return false;
            }
            if downloaded != total && now_ms.saturating_sub(previous.at_ms) < WINDOW_MS {
                return false;
            }
        }
        self.last_emitted.insert(
            filename.to_string(),
            EmittedProgress {
                downloaded,
                at_ms: now_ms,
            },
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::DownloadResult;

    fn progress(filename: &str, downloaded: i64, total: i64) -> InstallEvent {
        InstallEvent::DownloadProgress {
            filename: filename.to_string(),
            downloaded,
            total,
        }
    }

    fn init(filename: &str) -> InstallEvent {
        InstallEvent::DownloadInit {
            filename: filename.to_string(),
            optional: false,
        }
    }

    fn completed(filename: &str) -> InstallEvent {
        InstallEvent::DownloadCompleted {
            filename: filename.to_string(),
            total: 100,
            result: DownloadResult::Success,
        }
    }

    fn retry(filename: &str) -> InstallEvent {
        InstallEvent::DownloadRetry {
            filename: filename.to_string(),
            resume: true,
        }
    }

    #[test]
    fn unchanged_byte_count_is_dropped() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 0));
        assert!(!throttle.admit(&progress("a.pkg", 10, 100), 500));
    }

    #[test]
    fn first_progress_for_unknown_file_is_emitted() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("new.pkg", 0, 100), 0));
    }

    #[test]
    fn changed_bytes_within_window_are_dropped() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(!throttle.admit(&progress("a.pkg", 20, 100), 1050));
        assert!(!throttle.admit(&progress("a.pkg", 30, 100), 1099));
    }

    #[test]
    fn changed_bytes_after_window_are_emitted() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&progress("a.pkg", 20, 100), 1100));
    }

    #[test]
    fn finished_download_is_emitted_inside_window() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&progress("a.pkg", 100, 100), 1010));
    }

    #[test]
    fn completed_evicts_state_so_next_progress_starts_fresh() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&completed("a.pkg"), 1010));
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1020));
    }

    #[test]
    fn completed_passes_through_immediately() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&completed("a.pkg"), 0));
        assert!(throttle.admit(&completed("a.pkg"), 0));
    }

    #[test]
    fn retry_passes_through_without_touching_progress_state() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&retry("a.pkg"), 1010));
        assert!(!throttle.admit(&progress("a.pkg", 10, 100), 1020));
        assert!(!throttle.admit(&progress("a.pkg", 20, 100), 1030));
        assert!(throttle.admit(&progress("a.pkg", 20, 100), 1100));
    }

    #[test]
    fn init_resets_window_for_the_same_filename() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&init("a.pkg"), 1010));
        assert!(throttle.admit(&progress("a.pkg", 20, 100), 1020));
    }

    #[test]
    fn init_passes_through_immediately() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&init("a.pkg"), 0));
    }

    #[test]
    fn filenames_keep_independent_windows() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&progress("b.pkg", 10, 100), 1010));
        assert!(!throttle.admit(&progress("a.pkg", 20, 100), 1020));
        assert!(!throttle.admit(&progress("b.pkg", 20, 100), 1020));
        assert!(throttle.admit(&progress("a.pkg", 20, 100), 1100));
        assert!(!throttle.admit(&progress("b.pkg", 20, 100), 1105));
        assert!(throttle.admit(&progress("b.pkg", 20, 100), 1110));
    }

    #[test]
    fn duplicate_final_tick_is_dropped() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 100, 100), 1000));
        assert!(!throttle.admit(&progress("a.pkg", 100, 100), 1010));
    }

    #[test]
    fn init_eviction_allows_same_bytes_to_emit() {
        let mut throttle = DownloadThrottle::new();
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1000));
        assert!(throttle.admit(&init("a.pkg"), 1010));
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 1020));
    }

    #[test]
    fn other_event_variants_pass_through_untouched() {
        let mut throttle = DownloadThrottle::new();
        let other = InstallEvent::ProcessingChanges;
        assert!(throttle.admit(&other, 0));
        assert!(throttle.admit(&progress("a.pkg", 10, 100), 0));
    }
}
