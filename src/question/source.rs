use serde::{Deserialize, Serialize};

use super::model::{Answer, Question, QuestionKey};

pub trait AnswerSource {
    fn answer(&self, question: &Question) -> SourceDecision;
}

#[derive(Debug, Clone)]
pub enum SourceDecision {
    Answer(Answer),
    Abort(FailClosed),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailClosed {
    pub key: QuestionKey,
    pub reason: String,
}

impl FailClosed {
    pub fn new(key: QuestionKey, reason: impl Into<String>) -> Self {
        Self {
            key,
            reason: reason.into(),
        }
    }
}

pub fn parse_provider_selection(input: &str, candidate_count: usize) -> Option<usize> {
    if candidate_count == 0 {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(1);
    }
    match trimmed.parse::<usize>() {
        Ok(n) if (1..=candidate_count).contains(&n) => Some(n),
        _ => None,
    }
}

pub fn parse_group_selection(input: &str, member_count: usize) -> Option<Vec<usize>> {
    if input.trim().is_empty() {
        return Some((1..=member_count).collect());
    }
    let mut selected = Vec::new();
    for token in input.split_whitespace() {
        for n in parse_group_token(token, member_count)? {
            if !selected.contains(&n) {
                selected.push(n);
            }
        }
    }
    Some(selected)
}

fn parse_group_token(token: &str, member_count: usize) -> Option<Vec<usize>> {
    let Some((start, end)) = token.split_once('-') else {
        return parse_member_number(token, member_count).map(|n| vec![n]);
    };
    let lo = parse_member_number(start, member_count)?;
    let hi = parse_member_number(end, member_count)?;
    if lo > hi {
        return None;
    }
    Some((lo..=hi).collect())
}

fn parse_member_number(text: &str, member_count: usize) -> Option<usize> {
    match text.parse::<usize>() {
        Ok(n) if (1..=member_count).contains(&n) => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_selection_defaults_rejects_out_of_range() {
        let cases = [
            ("", 3, Some(1)),
            ("2", 3, Some(2)),
            ("x", 3, None),
            ("0", 3, None),
            ("4", 3, None),
            ("", 0, None),
        ];
        for (input, count, expected) in cases {
            assert_eq!(parse_provider_selection(input, count), expected);
        }
    }

    #[test]
    fn group_selection_expands_ranges_rejects_bad_tokens() {
        let cases = [
            ("", Some(vec![1, 2, 3, 4, 5])),
            ("3", Some(vec![3])),
            ("1-3 5", Some(vec![1, 2, 3, 5])),
            ("9", None),
            ("1-x", None),
            ("2 9", None),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_group_selection(input, 5), expected);
        }
    }
}
