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

pub fn parse_provider_selection(input: &str, candidate_count: usize) -> Option<usize> {
    if candidate_count == 0 {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(1);
    }
    parse_member_number(trimmed, candidate_count)
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
    fn selection_parsing() {
        for (input, count, expected) in [
            ("", 3, Some(1)),
            ("2", 3, Some(2)),
            ("3", 3, Some(3)),
            ("x", 3, None),
            ("0", 3, None),
            ("4", 3, None),
            ("", 0, None),
        ] {
            assert_eq!(parse_provider_selection(input, count), expected);
        }
        for (input, count, expected) in [
            ("", 5, Some(vec![1, 2, 3, 4, 5])),
            ("3", 5, Some(vec![3])),
            ("1-3 5", 5, Some(vec![1, 2, 3, 5])),
            ("1-3 2", 5, Some(vec![1, 2, 3])),
            ("3-1", 5, None),
            ("9", 5, None),
            ("1-x", 5, None),
            ("2 9", 5, None),
            ("", 0, Some(vec![])),
        ] {
            assert_eq!(parse_group_selection(input, count), expected);
        }
    }
}
