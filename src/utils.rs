use std::path::PathBuf;

use anyhow::Context as _;

pub fn cache_base() -> anyhow::Result<PathBuf> {
    match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) => Ok(PathBuf::from(xdg)),
        Err(_) => {
            let home =
                std::env::var("HOME").context("no cache directory: set XDG_CACHE_HOME or HOME")?;
            Ok(PathBuf::from(home).join(".cache"))
        }
    }
}

pub fn cache_root() -> anyhow::Result<PathBuf> {
    Ok(cache_base()?.join("pakajo"))
}

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

pub fn group_thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
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

pub fn humanize_age(secs: u64) -> String {
    if secs < 60 {
        "now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else if secs < 604800 {
        format!("{}d", secs / 86400)
    } else if secs < 2592000 {
        format!("{}w", secs / 604800)
    } else if secs < 31536000 {
        format!("{}mo", secs / 2592000)
    } else {
        format!("{}yr", secs / 31536000)
    }
}

pub fn format_elapsed(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = total % 3600 / 60;
    let seconds = total % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
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
    fn humanize_age_boundaries() {
        assert_eq!(humanize_age(0), "now");
        assert_eq!(humanize_age(59), "now");
        assert_eq!(humanize_age(60), "1m");
        assert_eq!(humanize_age(300), "5m");
        assert_eq!(humanize_age(3540), "59m");
        assert_eq!(humanize_age(3600), "1h");
        assert_eq!(humanize_age(7200), "2h");
        assert_eq!(humanize_age(86399), "23h");
        assert_eq!(humanize_age(86400), "1d");
        assert_eq!(humanize_age(604700), "6d");
        assert_eq!(humanize_age(604800), "1w");
        assert_eq!(humanize_age(2563200), "4w");
        assert_eq!(humanize_age(2592000), "1mo");
        assert_eq!(humanize_age(3024000), "1mo");
        assert_eq!(humanize_age(25920000), "10mo");
        assert_eq!(humanize_age(157680000), "5yr");
    }

    #[test]
    fn format_elapsed_display() {
        let cases = [
            (std::time::Duration::ZERO, "00:00"),
            (std::time::Duration::from_secs(7), "00:07"),
            (std::time::Duration::from_secs(247), "04:07"),
            (std::time::Duration::from_secs(3599), "59:59"),
            (std::time::Duration::from_secs(3600), "1:00:00"),
            (std::time::Duration::from_secs(3723), "1:02:03"),
            (std::time::Duration::from_secs(7384), "2:03:04"),
        ];
        for (input, expected) in cases {
            assert_eq!(format_elapsed(input), expected);
        }
    }

    #[test]
    fn group_thousands_digit_grouping() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(1234567), "1,234,567");
        assert_eq!(group_thousands(12884901), "12,884,901");
        assert_eq!(group_thousands(-1234567), "-1,234,567");
    }

    #[test]
    fn format_eta_display() {
        assert_eq!(format_eta(0), "00:00");
        assert_eq!(format_eta(65), "01:05");
        assert_eq!(format_eta(3600), "60:00");
        assert_eq!(format_eta(7200), "--:--");
    }
}
