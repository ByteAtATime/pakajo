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

pub(crate) fn version_diff(old: &str, new: &str) -> (String, String, String) {
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
}
