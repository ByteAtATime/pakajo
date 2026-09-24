fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    match_here(&pattern, &name)
}

fn match_here(pattern: &[char], name: &[char]) -> bool {
    let Some((&pc, rest)) = pattern.split_first() else {
        return name.is_empty();
    };
    match pc {
        '*' => match_star(rest, name),
        '?' => !name.is_empty() && match_here(rest, &name[1..]),
        '[' => match_class(pattern, name),
        _ => name.first() == Some(&pc) && match_here(rest, &name[1..]),
    }
}

fn match_star(pattern: &[char], name: &[char]) -> bool {
    (0..=name.len()).any(|split| match_here(pattern, &name[split..]))
}

fn match_class(pattern: &[char], name: &[char]) -> bool {
    let Some((&nc, rest_name)) = name.split_first() else {
        return false;
    };
    let mut class = &pattern[1..];
    let mut negated = false;
    if let Some((_marker @ ('!' | '^'), after)) = class.split_first() {
        negated = true;
        class = after;
    }
    let mut matched = false;
    let mut closed = false;
    let mut seen = 0;
    while let Some((&item, tail)) = class.split_first() {
        if item == ']' && seen > 0 {
            closed = true;
            class = tail;
            break;
        }
        seen += 1;
        class = match tail {
            ['-', high, rest @ ..] if *high != ']' => {
                matched |= item <= nc && nc <= *high;
                rest
            }
            _ => {
                matched |= item == nc;
                tail
            }
        };
    }
    if !closed {
        return nc == '[' && match_here(&pattern[1..], rest_name);
    }
    matched != negated && match_here(class, rest_name)
}

pub fn held_packages(names: &[String], patterns: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| patterns.iter().any(|p| glob_match(p, name)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_truth_table() {
        let cases = [
            ("glibc", "glibc", true),
            ("glibc", "glibc32", false),
            ("glibc32", "glibc", false),
            ("gl*", "glibc", true),
            ("*", "anything", true),
            ("g*c", "glibc", true),
            ("gl*", "pacman", false),
            ("s?", "sl", true),
            ("s?", "s", false),
            ("s?", "sls", false),
            ("sl[0-9]", "sl1", true),
            ("sl[0-9]", "slx", false),
            ("[abc]bc", "abc", true),
            ("[abc]bc", "dbc", false),
            ("[!0-9]x", "ax", true),
            ("[!0-9]x", "1x", false),
            ("foo", "bar", false),
            ("", "bar", false),
            ("", "", true),
        ];
        for (pattern, name, expected) in cases {
            assert_eq!(
                glob_match(pattern, name),
                expected,
                "pattern {pattern:?} vs name {name:?}"
            );
        }
    }

    #[test]
    fn held_packages_preserves_names_order() {
        let names = vec!["pacman".to_string(), "glibc".to_string(), "sl".to_string()];
        let patterns = vec!["glibc".to_string(), "s?".to_string()];
        assert_eq!(
            held_packages(&names, &patterns),
            vec!["glibc".to_string(), "sl".to_string()]
        );
    }
}
