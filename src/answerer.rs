use std::io::Write as _;

use crate::color;
use crate::question::model::ProviderCandidate;

pub trait QuestionAnswerer {
    fn answer_provider(&self, depend: &str, candidates: &[ProviderCandidate]) -> ProviderDecision;
}

#[derive(Debug)]
pub enum ProviderDecision {
    Choose(usize),
    Decline,
    CannotPrompt,
}

fn parse_provider_choice(input: &str, candidate_count: usize) -> ProviderDecision {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ProviderDecision::Choose(0);
    }
    let Ok(n) = trimmed.parse::<usize>() else {
        return ProviderDecision::Decline;
    };
    if n >= 1 && n <= candidate_count {
        ProviderDecision::Choose(n - 1)
    } else {
        ProviderDecision::Decline
    }
}

pub struct StdioAnswerer {
    color: bool,
}

impl StdioAnswerer {
    pub fn new() -> Self {
        Self {
            color: color::stderr_color(),
        }
    }
}

impl Default for StdioAnswerer {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionAnswerer for StdioAnswerer {
    fn answer_provider(&self, depend: &str, candidates: &[ProviderCandidate]) -> ProviderDecision {
        if candidates.is_empty() {
            return ProviderDecision::Decline;
        }
        eprintln!(
            "{}",
            color::colon(
                self.color,
                &format!(
                    "There are {} providers available for {}:",
                    candidates.len(),
                    depend
                )
            )
        );
        for (i, c) in candidates.iter().enumerate() {
            let display = match &c.repo {
                Some(repo) => format!("{repo}/{}", c.name),
                None => c.name.clone(),
            };
            eprintln!("  [{}] {display}", i + 1);
        }
        eprint!(
            "{} ",
            color::colon(self.color, "Enter a number (default=1):")
        );
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return ProviderDecision::CannotPrompt;
        }
        parse_provider_choice(&input, candidates.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_choice_picks_default_on_empty() {
        assert!(matches!(
            parse_provider_choice("", 3),
            ProviderDecision::Choose(0)
        ));
    }

    #[test]
    fn parse_provider_choice_first_entry_maps_to_zero_index() {
        assert!(matches!(
            parse_provider_choice("1", 3),
            ProviderDecision::Choose(0)
        ));
    }

    #[test]
    fn parse_provider_choice_last_entry_maps_to_count_minus_one() {
        assert!(matches!(
            parse_provider_choice("3", 3),
            ProviderDecision::Choose(2)
        ));
    }

    #[test]
    fn parse_provider_choice_zero_is_declined() {
        assert!(matches!(
            parse_provider_choice("0", 3),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn parse_provider_choice_past_end_is_declined() {
        assert!(matches!(
            parse_provider_choice("4", 3),
            ProviderDecision::Decline
        ));
    }

    #[test]
    fn parse_provider_choice_non_numeric_is_declined() {
        assert!(matches!(
            parse_provider_choice("abc", 3),
            ProviderDecision::Decline
        ));
    }
}
