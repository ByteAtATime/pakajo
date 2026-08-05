pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} {}", UNITS[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn format_mib(bytes: i64) -> String {
    let mut val = bytes as f64 / 1048576.0;
    if val < 0.0 && val > -0.005 {
        val = 0.0;
    }
    format!("{val:.2} MiB")
}

pub fn terminal_winsize() -> libc::winsize {
    use std::os::unix::io::AsRawFd as _;
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let fd = std::io::stdout().as_raw_fd();
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.ws_col > 0 && ws.ws_row > 0 {
        ws
    } else {
        libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

pub fn terminal_cols() -> usize {
    terminal_winsize().ws_col as usize
}

pub fn humanize_size(bytes: i64) -> (f64, &'static str) {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    (value, UNITS[idx])
}

pub fn format_rate(value: f64) -> String {
    if value < 9.995 {
        format!("{value:>4.2}")
    } else if value < 99.95 {
        format!("{value:>4.1}")
    } else {
        format!("{value:>4.0}")
    }
}

pub fn format_eta(seconds: u64) -> String {
    let eta_h = seconds / 3600;
    let rem = seconds % 3600;
    let eta_m = rem / 60;
    let eta_s = rem % 60;
    if eta_h == 0 {
        format!("{eta_m:02}:{eta_s:02}")
    } else if eta_h == 1 && eta_m < 40 {
        format!("{:02}:{:02}", eta_m + 60, eta_s)
    } else {
        "--:--".to_string()
    }
}

pub fn version_diff(old: &str, new: &str) -> (String, String, String) {
    let mut split = old.len().min(new.len());
    for ((oi, oc), (_, nc)) in old.char_indices().zip(new.char_indices()) {
        if oc != nc {
            split = oi;
            break;
        }
    }
    (
        old[..split].to_string(),
        old[split..].to_string(),
        new[split..].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_diff_identical_returns_whole_common() {
        let (common, old_suffix, new_suffix) = version_diff("1.2.3", "1.2.3");
        assert_eq!(common, "1.2.3");
        assert_eq!(old_suffix, "");
        assert_eq!(new_suffix, "");
    }

    #[test]
    fn version_diff_real_upgrade_splits_at_divergence() {
        let (common, old_suffix, new_suffix) = version_diff("1.2.3", "1.2.4");
        assert_eq!(common, "1.2.");
        assert_eq!(old_suffix, "3");
        assert_eq!(new_suffix, "4");
    }

    #[test]
    fn version_diff_suffix_divergence_highlights_tail() {
        let (common, old_suffix, new_suffix) = version_diff("10.2.3", "11.2.3");
        assert_eq!(common, "1");
        assert_eq!(old_suffix, "0.2.3");
        assert_eq!(new_suffix, "1.2.3");
    }

    #[test]
    fn version_diff_strict_prefix_empties_shorter_suffix() {
        let (common, old_suffix, new_suffix) = version_diff("1.2", "1.2.3");
        assert_eq!(common, "1.2");
        assert_eq!(old_suffix, "");
        assert_eq!(new_suffix, ".3");
    }

    #[test]
    fn humanize_size_boundaries() {
        assert_eq!(humanize_size(0), (0.0, "B"));
        assert_eq!(humanize_size(1024), (1.0, "KiB"));
        assert_eq!(humanize_size(1536), (1.5, "KiB"));
        assert_eq!(humanize_size(1048576), (1.0, "MiB"));
    }

    #[test]
    fn format_rate_precision_tiers() {
        assert_eq!(format_rate(0.0), "0.00");
        assert_eq!(format_rate(9.99), "9.99");
        assert_eq!(format_rate(10.0), "10.0");
        assert_eq!(format_rate(99.9), "99.9");
        assert_eq!(format_rate(100.0), " 100");
    }

    #[test]
    fn format_eta_display() {
        assert_eq!(format_eta(0), "00:00");
        assert_eq!(format_eta(65), "01:05");
        assert_eq!(format_eta(3600), "60:00");
        assert_eq!(format_eta(7200), "--:--");
    }
}
