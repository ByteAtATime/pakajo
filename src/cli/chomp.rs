use crate::color;

pub(super) fn render(percent: i32, width: usize, colored: bool) -> String {
    let pct = percent.clamp(0, 100) as usize;
    let hash = pct * width / 100;
    let mut out = String::with_capacity(width * 8 + 2);
    out.push('[');
    for p in 0..width {
        if p < hash {
            out.push('-');
        } else if p == hash {
            let head = if pct % 2 == 0 { 'C' } else { 'c' };
            out.push_str(&color::paint(colored, color::YELLOW, &head.to_string()));
        } else {
            let i = width - p;
            if i % 3 == 0 {
                out.push_str(&color::paint(colored, color::WHITE, "o"));
            } else {
                out.push(' ');
            }
        }
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_progress_is_all_dashes() {
        assert_eq!(render(100, 10, false), "[----------]");
    }

    #[test]
    fn zero_progress_shows_open_mouth_at_head() {
        assert_eq!(&render(0, 10, false)[..2], "[C");
    }

    #[test]
    fn odd_percent_closes_mouth() {
        let bar = render(1, 30, false);
        assert!(bar.contains('c'));
    }
}
