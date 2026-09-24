use std::collections::HashMap;
use std::time::Instant;

use crate::events::InstallEvent;

pub const RATE_SAMPLE_MS: u128 = 200;
pub const ETA_UNKNOWN: u64 = u32::MAX as u64;

pub fn blend_rate(chunk_rate: f64, rate: f64) -> f64 {
    (chunk_rate + 2.0 * rate) / 3.0
}

#[derive(Debug, Clone)]
pub struct Meter {
    pub xfered: i64,
    pub total: i64,
    pub init_time: Instant,
    pub sync_xfered: i64,
    pub sync_time: Option<Instant>,
    pub rate: f64,
    pub eta: u64,
}

impl Meter {
    pub fn fresh(now: Instant) -> Self {
        Meter {
            xfered: 0,
            total: 0,
            init_time: now,
            sync_xfered: 0,
            sync_time: None,
            rate: 0.0,
            eta: 0,
        }
    }

    pub fn reset(&mut self, now: Instant) {
        self.xfered = 0;
        self.total = 0;
        self.init_time = now;
        self.sync_xfered = 0;
        self.sync_time = None;
        self.rate = 0.0;
        self.eta = 0;
    }

    fn derive_eta(&mut self, total: i64) {
        if self.rate > 0.0 {
            let remaining = (total - self.sync_xfered) as f64 / self.rate;
            self.eta = if remaining < 0.0 || remaining > ETA_UNKNOWN as f64 {
                ETA_UNKNOWN
            } else {
                remaining as u64
            };
        } else {
            self.eta = ETA_UNKNOWN;
        }
    }

    pub fn observe(&mut self, now: Instant, downloaded: i64, total: i64) -> bool {
        if downloaded < 0 || total < 0 {
            return false;
        }
        self.xfered = downloaded;
        self.total = total;
        match self.sync_time {
            None => {
                self.sync_xfered = downloaded;
                self.sync_time = Some(now);
                if downloaded > 0 {
                    self.rate = f64::MIN_POSITIVE;
                } else {
                    self.rate = 0.0;
                }
                self.derive_eta(total);
                true
            }
            Some(previous) => {
                let elapsed = now.saturating_duration_since(previous).as_millis();
                if elapsed < RATE_SAMPLE_MS {
                    return false;
                }
                let chunk = downloaded - self.sync_xfered;
                self.sync_xfered = downloaded;
                self.sync_time = Some(now);
                if chunk > 0 && elapsed > 0 {
                    let chunk_rate = chunk as f64 * 1000.0 / elapsed as f64;
                    self.rate = blend_rate(chunk_rate, self.rate);
                }
                self.derive_eta(total);
                true
            }
        }
    }

    pub fn finish(&mut self, now: Instant, total: i64) {
        let total = total.max(0);
        self.xfered = total;
        self.total = total;
        self.sync_xfered = total;
        let elapsed = now
            .saturating_duration_since(self.init_time)
            .as_millis()
            .max(1);
        self.rate = total as f64 * 1000.0 / elapsed as f64;
        self.eta = ((elapsed + 500) / 1000).min(u128::from(u64::MAX)) as u64;
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::fresh(Instant::now())
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileTransfer {
    pub downloaded: i64,
    pub total: i64,
    pub completed: bool,
    pub meter: Meter,
}

impl FileTransfer {
    pub fn fresh(now: Instant) -> Self {
        FileTransfer {
            downloaded: 0,
            total: 0,
            completed: false,
            meter: Meter::fresh(now),
        }
    }

    pub fn observe(&mut self, now: Instant, downloaded: i64, total: i64) -> bool {
        self.downloaded = downloaded;
        self.total = total;
        self.meter.observe(now, downloaded, total)
    }

    pub fn reset(&mut self, now: Instant) {
        self.downloaded = 0;
        self.total = 0;
        self.meter.reset(now);
    }

    pub fn complete(&mut self, total: i64) {
        if self.completed {
            return;
        }
        self.downloaded = total;
        self.total = total;
        self.completed = true;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Totals {
    pub howmany: usize,
    pub downloaded: usize,
    pub size: i64,
    pub xfered: i64,
    pub meter: Meter,
}

impl Totals {
    pub fn reset(&mut self, howmany: usize, size: i64, now: Instant) {
        self.howmany = howmany;
        self.downloaded = 0;
        self.size = size;
        self.xfered = 0;
        self.meter = Meter::fresh(now);
        self.meter.total = size;
        self.meter.eta = ETA_UNKNOWN;
    }

    pub fn add_chunk(&mut self, chunk: i64, now: Instant) -> bool {
        self.xfered = (self.xfered + chunk).max(0);
        self.meter.observe(now, self.xfered, self.size)
    }

    pub fn rollback(&mut self, amount: i64) {
        if amount <= 0 {
            return;
        }
        self.xfered = (self.xfered - amount).max(0);
        self.meter.xfered = (self.meter.xfered - amount).max(0);
        self.meter.sync_xfered = (self.meter.sync_xfered - amount).max(0);
    }

    pub fn count_completion(&mut self) {
        self.downloaded += 1;
    }

    pub fn finish(&mut self, now: Instant) {
        self.meter.finish(now, self.size);
    }
}

#[derive(Debug, Clone, Default)]
pub struct TransferState {
    pub total: usize,
    pub done: usize,
    pub bytes_total: i64,
    pub bytes_done: i64,
    pub files: HashMap<String, FileTransfer>,
    pub order: Vec<String>,
    pub totals: Totals,
}

impl TransferState {
    pub fn queued(&self) -> usize {
        let active = self.files.values().filter(|f| !f.completed).count();
        self.total.saturating_sub(self.done).saturating_sub(active)
    }

    pub fn apply(&mut self, ev: &InstallEvent, now: Instant) {
        match ev {
            InstallEvent::RetrievingPackages { num, total_bytes } => {
                self.total = *num;
                self.done = 0;
                self.bytes_total = *total_bytes;
                self.bytes_done = 0;
                self.files.clear();
                self.order.clear();
                self.totals.reset(*num, *total_bytes, now);
            }
            InstallEvent::DownloadInit { filename, .. } => {
                self.ensure_file(filename, now);
            }
            InstallEvent::DownloadProgress {
                filename,
                downloaded,
                total,
            } => {
                let previous = self.ensure_file(filename, now).downloaded;
                if let Some(entry) = self.files.get_mut(filename) {
                    entry.observe(now, *downloaded, *total);
                }
                let chunk = *downloaded - previous;
                self.bytes_done += chunk;
                self.totals.add_chunk(chunk, now);
            }
            InstallEvent::DownloadRetry { filename, resume } => {
                if !*resume && let Some(entry) = self.files.get_mut(filename) {
                    let previous = entry.downloaded;
                    entry.reset(now);
                    self.bytes_done -= previous;
                    self.totals.rollback(previous);
                }
            }
            InstallEvent::DownloadCompleted {
                filename, total, ..
            } => {
                self.ensure_file(filename, now).complete(*total);
                self.done += 1;
                self.totals.count_completion();
            }
            InstallEvent::PkgRetrieveDone { .. } | InstallEvent::PkgRetrieveFailed { .. } => {
                self.bytes_done = self.bytes_total;
                self.totals.finish(now);
            }
            _ => {}
        }
    }

    fn ensure_file(&mut self, filename: &str, now: Instant) -> &mut FileTransfer {
        if !self.files.contains_key(filename) {
            self.order.push(filename.to_string());
        }
        self.files
            .entry(filename.to_string())
            .or_insert_with(|| FileTransfer::fresh(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn download_progress_first_sample_shows_zero_rate_unknown_eta() {
        let base = Instant::now();
        let mut meter = Meter::fresh(base);
        assert!(meter.observe(base, 1000, 9000));
        assert!(meter.rate > 0.0);
        assert_eq!(meter.rate.trunc(), 0.0);
        assert_eq!(meter.eta, ETA_UNKNOWN);
        assert_eq!(meter.sync_xfered, 1000);
        assert_eq!(meter.sync_time, Some(base));
    }

    #[test]
    fn download_progress_first_sample_of_complete_file_shows_zero_eta() {
        let base = Instant::now();
        let mut meter = Meter::fresh(base);
        assert!(meter.observe(base, 742, 742));
        assert_eq!(meter.eta, 0);
    }

    #[test]
    fn download_progress_first_sample_of_empty_file_shows_unknown_eta() {
        let base = Instant::now();
        let mut meter = Meter::fresh(base);
        assert!(meter.observe(base, 0, 0));
        assert_eq!(meter.rate, 0.0);
        assert_eq!(meter.eta, ETA_UNKNOWN);
    }

    #[test]
    fn download_progress_second_sample_computes_rate_and_eta() {
        let base = Instant::now();
        let mut meter = Meter::fresh(base);
        assert!(meter.observe(base, 1000, 9000));
        assert!(!meter.observe(base + Duration::from_millis(100), 2000, 9000));
        assert!(meter.observe(base + Duration::from_millis(300), 3000, 9000));
        let chunk_rate = 2000.0 * 1000.0 / 300.0;
        let expected_rate = (chunk_rate + 2.0 * 0.0) / 3.0;
        assert_eq!(meter.rate, expected_rate);
        assert_eq!(meter.eta, ((9000 - 3000) as f64 / expected_rate) as u64);
        assert_eq!(meter.sync_xfered, 3000);
        assert_eq!(meter.sync_time, Some(base + Duration::from_millis(300)));
    }
}
