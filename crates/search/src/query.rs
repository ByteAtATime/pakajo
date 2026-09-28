#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedQuery {
    Short(String),
    Quoted(String),
    Normal(String),
}

impl ParsedQuery {
    pub fn text(&self) -> &str {
        match self {
            ParsedQuery::Short(s) | ParsedQuery::Quoted(s) | ParsedQuery::Normal(s) => s,
        }
    }
}

pub fn parse_query(text: &str) -> Option<ParsedQuery> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        let inner = rest.trim();
        if inner.is_empty() {
            return None;
        }
        return Some(ParsedQuery::Quoted(inner.to_lowercase()));
    }
    let q = trimmed.to_lowercase();
    let len = trimmed.chars().count();
    if len <= 2 {
        Some(ParsedQuery::Short(q))
    } else {
        Some(ParsedQuery::Normal(q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_only_returns_none() {
        assert_eq!(parse_query("   "), None);
    }

    #[test]
    fn empty_quoted_returns_none() {
        assert_eq!(parse_query("\"\""), None);
    }

    #[test]
    fn quoted_preserves_inner_spaces() {
        assert_eq!(
            parse_query("\"google chrome\""),
            Some(ParsedQuery::Quoted("google chrome".into()))
        );
    }

    #[test]
    fn quoted_strips_quotes_and_lowercases() {
        assert_eq!(
            parse_query("\"Vim\""),
            Some(ParsedQuery::Quoted("vim".into()))
        );
    }

    #[test]
    fn two_char_non_alphanumeric_is_short() {
        assert_eq!(parse_query("c+"), Some(ParsedQuery::Short("c+".into())));
    }

    #[test]
    fn three_char_is_normal() {
        assert_eq!(parse_query("abc"), Some(ParsedQuery::Normal("abc".into())));
    }

    #[test]
    fn normal_trims_and_lowercases() {
        assert_eq!(
            parse_query("  Vim  "),
            Some(ParsedQuery::Normal("vim".into()))
        );
    }

    #[test]
    fn unbalanced_quote_falls_through_to_normal() {
        assert_eq!(
            parse_query("\"ab"),
            Some(ParsedQuery::Normal("\"ab".into()))
        );
    }
}
