use crate::search::index::byte_mask;

pub const MAX_EDIT_DISTANCE: usize = 2;

pub struct FuzzyMatcher<'q> {
    q: &'q [u8],
    qmask: u64,
    prev_prev: Vec<usize>,
    prev: Vec<usize>,
    cur: Vec<usize>,
}

impl<'q> FuzzyMatcher<'q> {
    pub fn new(q: &'q [u8]) -> Self {
        let m = q.len();
        let prev_prev: Vec<usize> = vec![0usize; m + 1];
        let prev: Vec<usize> = (0..=m).collect();
        let cur: Vec<usize> = vec![0usize; m + 1];
        Self {
            q,
            qmask: byte_mask(q),
            prev_prev,
            prev,
            cur,
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
        if (self.qmask & !cmask).count_ones() as usize > max {
            return None;
        }
        if both_ascii {
            return self.within_ascii_dist(cand, max);
        }
        let q_str = std::str::from_utf8(self.q).expect("query bytes are valid utf-8");
        let cand_str = std::str::from_utf8(cand).expect("candidate bytes are valid utf-8");
        edit_distance_chars(cand_str, q_str, max)
    }

    fn within_ascii_dist(&mut self, cand: &[u8], max: usize) -> Option<usize> {
        let q = self.q;
        let n = cand.len();
        let m = q.len();
        for j in 0..=m {
            self.prev[j] = j;
        }
        for i in 1..=n {
            self.cur[0] = i;
            let mut row_min = self.cur[0];
            for j in 1..=m {
                let cost = if cand[i - 1] == q[j - 1] { 0 } else { 1 };
                let del = self.prev[j] + 1;
                let ins = self.cur[j - 1] + 1;
                let sub = self.prev[j - 1] + cost;
                self.cur[j] = del.min(ins).min(sub);
                if i >= 2 && j >= 2 && cand[i - 1] == q[j - 2] && cand[i - 2] == q[j - 1] {
                    self.cur[j] = self.cur[j].min(self.prev_prev[j - 2] + 1);
                }
                if self.cur[j] < row_min {
                    row_min = self.cur[j];
                }
            }
            if row_min > max {
                return None;
            }
            std::mem::swap(&mut self.prev_prev, &mut self.prev);
            std::mem::swap(&mut self.prev, &mut self.cur);
        }
        let d = self.prev[m];
        (d <= max).then_some(d)
    }
}

fn edit_distance_chars(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > max {
        return None;
    }
    let mut prev_prev: Vec<usize> = vec![0usize; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let del = prev[j] + 1;
            let ins = cur[j - 1] + 1;
            let sub = prev[j - 1] + cost;
            cur[j] = del.min(ins).min(sub);
            if i >= 2 && j >= 2 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev_prev[j - 2] + 1);
            }
            if cur[j] < row_min {
                row_min = cur[j];
            }
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[m];
    (d <= max).then_some(d)
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
}
