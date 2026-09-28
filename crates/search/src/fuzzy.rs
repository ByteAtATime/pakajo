use crate::index::byte_mask;

pub const MAX_EDIT_DISTANCE: usize = 2;

pub(crate) fn at_most_two_missing(miss: u64) -> bool {
    let m1 = miss & miss.wrapping_sub(1);
    m1 & m1.wrapping_sub(1) == 0
}

struct Rows {
    prev_prev: Vec<usize>,
    prev: Vec<usize>,
    cur: Vec<usize>,
}

impl Rows {
    fn new(m: usize) -> Self {
        Self {
            prev_prev: vec![0; m + 1],
            prev: (0..=m).collect(),
            cur: vec![0; m + 1],
        }
    }

    fn reset(&mut self, m: usize) {
        self.prev_prev.clear();
        self.prev_prev.resize(m + 1, 0);
        self.cur.clear();
        self.cur.resize(m + 1, 0);
        for (j, slot) in self.prev.iter_mut().enumerate() {
            *slot = j;
        }
    }
}

pub struct FuzzyMatcher<'q> {
    q: &'q [u8],
    qmask: u64,
    rows: Rows,
}

impl<'q> FuzzyMatcher<'q> {
    pub fn new(q: &'q [u8]) -> Self {
        Self {
            q,
            qmask: byte_mask(q),
            rows: Rows::new(q.len()),
        }
    }

    pub fn within_distance(&mut self, cand: &[u8], cmask: u64, max: usize) -> Option<usize> {
        if cand == self.q {
            return Some(0);
        }
        let both_ascii = self.q.is_ascii() && cand.is_ascii();
        if both_ascii && cand.len().abs_diff(self.q.len()) > max {
            return None;
        }
        let miss = self.qmask & !cmask;
        if max == MAX_EDIT_DISTANCE {
            if !at_most_two_missing(miss) {
                return None;
            }
        } else if miss.count_ones() as usize > max {
            return None;
        }
        if both_ascii {
            return self.run(cand, self.q, max);
        }
        let q_str = std::str::from_utf8(self.q).expect("query bytes are valid utf-8");
        let cand_str = std::str::from_utf8(cand).expect("candidate bytes are valid utf-8");
        let q_chars: Vec<char> = q_str.chars().collect();
        let cand_chars: Vec<char> = cand_str.chars().collect();
        self.run(&cand_chars, &q_chars, max)
    }

    fn run<T: Copy + PartialEq>(&mut self, cand: &[T], q: &[T], max: usize) -> Option<usize> {
        let (n, m) = (cand.len(), q.len());
        if n.abs_diff(m) > max {
            return None;
        }
        self.rows.reset(m);
        for i in 1..=n {
            let rows = &mut self.rows;
            rows.cur[0] = i;
            let mut row_min = i;
            for j in 1..=m {
                let cost = usize::from(cand[i - 1] != q[j - 1]);
                let mut best = (rows.prev[j] + 1)
                    .min(rows.cur[j - 1] + 1)
                    .min(rows.prev[j - 1] + cost);
                if i >= 2 && j >= 2 && cand[i - 1] == q[j - 2] && cand[i - 2] == q[j - 1] {
                    best = best.min(rows.prev_prev[j - 2] + 1);
                }
                rows.cur[j] = best;
                row_min = row_min.min(best);
            }
            if row_min > max {
                return None;
            }
            std::mem::swap(&mut self.rows.prev_prev, &mut self.rows.prev);
            std::mem::swap(&mut self.rows.prev, &mut self.rows.cur);
        }
        (self.rows.prev[m] <= max).then_some(self.rows.prev[m])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(a: &str, b: &str, max: usize) -> bool {
        FuzzyMatcher::new(b.as_bytes())
            .within_distance(a.as_bytes(), byte_mask(a.as_bytes()), max)
            .is_some()
    }

    #[test]
    fn damerau_counts_transposition_as_one() {
        assert!(within("chrome", "chroem", 1));
    }

    #[test]
    fn edit_distance_single_sub() {
        assert!(within("chrome", "chromm", 2));
    }

    #[test]
    fn edit_distance_too_far() {
        assert!(!within("chrome", "chromium", 2));
    }

    #[test]
    fn edit_distance_length_cutoff() {
        assert!(!within("a", "abcd", 2));
    }

    #[test]
    fn non_ascii_compares_chars_not_bytes() {
        assert!(within("über", "yber", 2));
        assert!(!within("über", "zzzzzz", 2));
    }

    #[test]
    fn matcher_shares_its_rows_between_the_byte_and_char_paths() {
        let mut matcher = FuzzyMatcher::new("über".as_bytes());
        for _ in 0..3 {
            assert_eq!(
                matcher.within_distance(b"yber", byte_mask(b"yber"), 2),
                Some(1)
            );
            assert_eq!(
                matcher.within_distance("über".as_bytes(), byte_mask("über".as_bytes()), 2),
                Some(0)
            );
            assert_eq!(
                matcher.within_distance(b"qqqqqqqq", byte_mask(b"qqqqqqqq"), 2),
                None
            );
        }
    }
}
