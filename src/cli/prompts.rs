use std::io::Write as _;

use crate::package::PackageGroup;
use crate::resolve::{Ask, Conflict, ConflictReport, GroupMember, Plan};
use crate::{build::BuildDecision, color};

pub(crate) enum PromptStream {
    Stdout,
}

fn read_confirmation(message: &str, stream: PromptStream, default_yes: bool) -> bool {
    let c = match stream {
        PromptStream::Stdout => color::stdout_color(),
    };
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let line = format!(
        "{} {} ",
        color::colon(c, message),
        color::paint(c, color::BOLD, hint)
    );
    match stream {
        PromptStream::Stdout => {
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    parse_confirmation(&input, default_yes)
}

fn parse_confirmation(input: &str, default_yes: bool) -> bool {
    match input.trim().to_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

fn select_group_indices(group_name: &str, groups: &[crate::package::PackageGroup]) -> Vec<usize> {
    let c = color::stdout_color();
    let flat: Vec<&crate::package::GroupMember> =
        groups.iter().flat_map(|g| g.members.iter()).collect();
    let total = flat.len();
    println!(
        "{}",
        color::colon(
            c,
            &format!("There are {total} members in group {group_name}:")
        )
    );

    let mut n = 1usize;
    let mut current_repo = "";
    for group in groups {
        let db_name = group.repo.as_str();
        if db_name != current_repo {
            current_repo = db_name;
            println!("{}", color::colon(c, &format!("Repository {db_name}")));
        }
        let mut line = String::from("    ");
        for member in group.members.iter() {
            line.push_str(&format!("{n}) {}  ", member.name));
            n += 1;
        }
        println!("{}", line.trim_end());
    }

    loop {
        print!(
            "\n{}",
            color::paint(c, color::BOLD, "Enter a selection (default=all):")
        );
        print!(" ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return Vec::new();
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return (0..total).collect();
        }
        let mut picks = Vec::new();
        let mut invalid: Option<&str> = None;
        for tok in trimmed.split_whitespace() {
            match tok.parse::<usize>() {
                Ok(num) if (1..=total).contains(&num) => picks.push(num - 1),
                _ => {
                    invalid = Some(tok);
                    break;
                }
            }
        }
        if let Some(tok) = invalid {
            println!("error: invalid number: {tok}");
            continue;
        }
        return picks;
    }
}

fn parse_provider_choice(input: &str, candidate_count: usize) -> Option<usize> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    let Ok(n) = trimmed.parse::<usize>() else {
        return None;
    };
    if (1..=candidate_count).contains(&n) {
        Some(n - 1)
    } else {
        None
    }
}

fn render_provider_menu(depend: &str, candidates: &[String]) {
    let colored = color::stderr_color();
    eprintln!(
        "{}",
        color::colon(
            colored,
            &format!(
                "There are {} providers available for {}:",
                candidates.len(),
                depend
            )
        )
    );
    for (index, name) in candidates.iter().enumerate() {
        eprintln!("  [{}] {name}", index + 1);
    }
    eprint!("{} ", color::colon(colored, "Enter a number (default=1):"));
    let _ = std::io::stderr().flush();
}

fn provider_index_from_reader<R: std::io::BufRead>(
    depend: &str,
    candidates: &[String],
    reader: &mut R,
) -> usize {
    loop {
        render_provider_menu(depend, candidates);
        let mut input = String::new();
        if reader.read_line(&mut input).is_err() {
            return 0;
        }
        if let Some(index) = parse_provider_choice(&input, candidates.len()) {
            return index;
        }
    }
}

pub(crate) struct CliAsk;

fn group_packages(members: &[GroupMember]) -> Vec<PackageGroup> {
    let mut groups: Vec<PackageGroup> = Vec::new();
    for member in members {
        if !groups.last().is_some_and(|last| last.repo == member.db) {
            groups.push(PackageGroup {
                repo: member.db.clone(),
                members: Vec::new(),
            });
        }
        groups
            .last_mut()
            .expect("group pushed above")
            .members
            .push(crate::package::GroupMember {
                name: member.name.clone(),
                description: None,
            });
    }
    groups
}

impl Ask for CliAsk {
    fn choose_provider(&mut self, depend: &str, candidates: &[String]) -> usize {
        if candidates.is_empty() {
            return 0;
        }
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        provider_index_from_reader(depend, candidates, &mut locked)
    }

    fn choose_group_members(&mut self, group: &str, members: &[GroupMember]) -> Vec<usize> {
        if members.is_empty() {
            return Vec::new();
        }
        select_group_indices(group, &group_packages(members))
    }
}

pub fn confirm_install() -> bool {
    read_confirmation("Proceed with installation?", PromptStream::Stdout, true)
}

fn member_label(make: bool, target: bool) -> Option<&'static str> {
    match (make, target) {
        (_, true) => None,
        (true, false) => Some("makedepend"),
        (false, false) => Some("dependency"),
    }
}

fn print_plan_summary(plan: &Plan) {
    let mut rows: Vec<(&str, &str, Option<&str>)> = plan
        .repo_installs
        .iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.version.as_str(),
                member_label(row.make, row.target),
            )
        })
        .collect();
    rows.extend(plan.all_members().map(|member| {
        (
            member.name.as_str(),
            member.version.as_str(),
            member_label(member.make, member.target),
        )
    }));

    let name_width = rows
        .iter()
        .map(|(name, _, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let version_width = rows
        .iter()
        .map(|(_, version, _)| version.chars().count())
        .max()
        .unwrap_or(0);

    println!();
    let c = color::stdout_color();
    for (name, version, label) in &rows {
        let v = color::paint(c, color::VERSION, version);
        let vw = version_width + v.chars().count().saturating_sub(color::visible_width(&v));
        match label {
            Some(l) => println!(
                "  {:<nw$}  {:<vw$}  ({l})",
                name,
                v,
                nw = name_width,
                vw = vw,
            ),
            None => println!("  {:<nw$}  {:<vw$}", name, v, nw = name_width, vw = vw,),
        }
    }
    println!();

    let aur_count = plan.all_members().count();
    let repo_dep_count: usize = plan.repo_installs.len();
    let aur_word = if aur_count == 1 {
        "package"
    } else {
        "packages"
    };
    if repo_dep_count > 0 {
        println!(
            "{}",
            color::colon(
                c,
                &format!("{aur_count} {aur_word} to build, {repo_dep_count} to install")
            )
        );
    } else {
        println!(
            "{}",
            color::colon(c, &format!("{aur_count} {aur_word} to build"))
        );
    }
}

pub fn confirm_proceed_to_review(plan: &Plan) -> BuildDecision {
    print_plan_summary(plan);
    if read_confirmation("Proceed to review?", PromptStream::Stdout, true) {
        BuildDecision::Review
    } else {
        BuildDecision::Abort
    }
}

pub fn announce_conflict_calculation() {
    let c = color::stderr_color();
    for message in ["Calculating conflicts...", "Calculating inner conflicts..."] {
        eprintln!(
            "{} {}",
            color::paint(c, color::RED, "::"),
            color::paint(c, color::BOLD, message)
        );
    }
}

pub fn print_conflicts(report: &ConflictReport) {
    eprintln!();
    print_conflict_section("Inner conflicts found:", &report.inner);
    print_conflict_section("Conflicts found:", &report.local);
}

pub fn confirm_proceed_install(plan: &Plan) -> BuildDecision {
    print_plan_summary(plan);
    if confirm_install() {
        BuildDecision::Proceed
    } else {
        BuildDecision::Abort
    }
}

pub fn confirm_conflicts(_report: &ConflictReport) -> bool {
    confirm_install()
}

pub fn confirm_conflict_warning(non_interactive: bool) {
    if !non_interactive {
        return;
    }
    let c = color::stderr_color();
    eprintln!(
        "{} {}",
        color::paint(c, color::YELLOW, "::"),
        color::paint(
            c,
            color::BOLD,
            "Conflicting packages will have to be confirmed manually"
        )
    );
}

fn print_conflict_section(title: &str, items: &[Conflict]) {
    if items.is_empty() {
        return;
    }
    let c = color::stderr_color();
    eprintln!(
        "{} {}",
        color::paint(c, color::RED, "::"),
        color::paint(c, color::BOLD, title)
    );
    for conflict in items {
        let details = conflict
            .conflicting
            .iter()
            .map(|conflicting| match &conflicting.conflict {
                Some(detail) => format!("{} ({detail})", conflicting.pkg),
                None => conflicting.pkg.clone(),
            })
            .collect::<Vec<_>>()
            .join("  ");
        eprintln!("    {}: {details}", conflict.pkg);
    }
    eprintln!();
}

pub fn confirm_review_accept() -> bool {
    println!();
    read_confirmation("Accept changes?", PromptStream::Stdout, true)
}

#[cfg(test)]
mod tests {
    use super::{CliAsk, parse_confirmation, parse_provider_choice, provider_index_from_reader};
    use crate::resolve::Ask;
    use std::io::Cursor;

    #[test]
    fn confirmation_answers_proceed_and_decline() {
        for (input, default_yes, expected) in [
            ("y", false, true),
            ("yes", false, true),
            ("Y", false, true),
            ("n", true, false),
            ("no", true, false),
            ("", true, true),
            ("", false, false),
        ] {
            assert_eq!(parse_confirmation(input, default_yes), expected);
        }
    }

    #[test]
    fn parse_provider_choice_maps_entries_and_declines_out_of_range() {
        for (input, expected) in [
            ("", Some(0)),
            ("1", Some(0)),
            ("3", Some(2)),
            ("0", None),
            ("4", None),
            ("abc", None),
        ] {
            assert_eq!(parse_provider_choice(input, 3), expected);
        }
    }

    #[test]
    fn provider_prompt_reads_choice_from_reader() {
        let candidates = vec![String::from("a"), String::from("b")];
        let mut reader = Cursor::new("2\n");
        assert_eq!(provider_index_from_reader("x", &candidates, &mut reader), 1);
    }

    #[test]
    fn provider_prompt_empty_input_selects_default() {
        let candidates = vec![String::from("a"), String::from("b")];
        let mut reader = Cursor::new("");
        assert_eq!(provider_index_from_reader("x", &candidates, &mut reader), 0);
    }

    struct FailingReader;

    impl std::io::BufRead for FailingReader {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("boom"))
        }
        fn consume(&mut self, _amt: usize) {}
    }

    impl std::io::Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }

    #[test]
    fn provider_prompt_read_error_selects_default() {
        let candidates = vec![String::from("a"), String::from("b")];
        let mut reader = FailingReader;
        assert_eq!(provider_index_from_reader("x", &candidates, &mut reader), 0);
    }

    #[test]
    fn choose_provider_with_no_candidates_selects_default() {
        assert_eq!(CliAsk.choose_provider("x", &[]), 0);
    }
}
