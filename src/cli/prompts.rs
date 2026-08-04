use std::io::Write as _;

use crate::{build::BuildDecision, color, resolve::BuildPlan};

enum PromptStream {
    Stdout,
    Stderr,
}

fn read_confirmation(message: &str, stream: PromptStream) -> bool {
    let c = match stream {
        PromptStream::Stdout => color::stdout_color(),
        PromptStream::Stderr => color::stderr_color(),
    };
    let line = format!(
        "{} {} ",
        color::colon(c, message),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    match stream {
        PromptStream::Stdout => {
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
        PromptStream::Stderr => {
            eprint!("{line}");
            let _ = std::io::stderr().flush();
        }
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

pub(super) fn select_group_members(
    group_name: &str,
    groups: &[(&alpm::Db, &alpm::Group)],
) -> Vec<String> {
    let c = color::stdout_color();
    let flat: Vec<&alpm::Package> = groups
        .iter()
        .flat_map(|(_, g)| g.packages().iter())
        .collect();
    let total = flat.len();
    println!(
        "{}",
        color::colon(c, &format!("There are {total} members in group {group_name}:"))
    );

    let mut n = 1usize;
    let mut current_repo = "";
    for (db, group) in groups {
        let db_name = db.name();
        if db_name != current_repo {
            current_repo = db_name;
            println!("{}", color::colon(c, &format!("Repository {db_name}")));
        }
        let mut line = String::from("    ");
        for pkg in group.packages().iter() {
            line.push_str(&format!("{n}) {}  ", pkg.name()));
            n += 1;
        }
        println!("{}", line.trim_end());
    }

    loop {
        print!("\n{}", color::paint(c, color::BOLD, "Enter a selection (default=all):"));
        print!(" ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return flat.iter().map(|p| p.name().to_string()).collect();
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return flat.iter().map(|p| p.name().to_string()).collect();
        }
        let mut picks: Vec<usize> = Vec::new();
        let mut had_error = false;
        for tok in trimmed.split_whitespace() {
            match tok.parse::<usize>() {
                Ok(num) if num >= 1 && num <= total => picks.push(num - 1),
                _ => {
                    println!("error: invalid number: {tok}");
                    had_error = true;
                }
            }
        }
        if had_error {
            continue;
        }
        return picks.into_iter().map(|i| flat[i].name().to_string()).collect();
    }
}

pub(super) fn confirm_install() -> bool {
    print!("\n");
    read_confirmation("Proceed with installation?", PromptStream::Stdout)
}

pub(super) fn confirm_remove() -> bool {
    print!("\n");
    read_confirmation("Do you want to remove these packages?", PromptStream::Stdout)
}

fn print_plan_summary(plan: &BuildPlan) {
    let rows: Vec<(&str, &str, Option<&str>)> = plan
        .layers
        .iter()
        .flat_map(|layer| layer.aur.iter())
        .map(|info| {
            let label = if plan.targets.iter().any(|t| t == &info.name) {
                None
            } else {
                Some("dependency")
            };
            (info.name.as_str(), info.version.as_str(), label)
        })
        .collect();

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

    let aur_count = rows.len();
    let repo_dep_count: usize = plan.layers.iter().map(|l| l.repo_deps.len()).sum();
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

pub(super) fn confirm_build(plan: &BuildPlan) -> BuildDecision {
    print_plan_summary(plan);
    let c = color::stdout_color();
    print!(
        "{} {} ",
        color::colon(c, "Proceed with build?"),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    match input.trim().to_lowercase().as_str() {
        "" | "y" | "yes" => BuildDecision::Proceed,
        _ => BuildDecision::Abort,
    }
}

pub(super) fn confirm_proceed_to_review(plan: &BuildPlan) -> BuildDecision {
    print_plan_summary(plan);
    if read_confirmation("Proceed to review?", PromptStream::Stdout) {
        BuildDecision::Review
    } else {
        BuildDecision::Abort
    }
}

pub(super) fn confirm_install_stderr() -> bool {
    eprint!("\n");
    read_confirmation("Proceed with installation?", PromptStream::Stderr)
}

pub(super) fn confirm_remove_stderr() -> bool {
    eprint!("\n");
    read_confirmation("Do you want to remove these packages?", PromptStream::Stderr)
}

pub(super) fn confirm_review_accept() -> bool {
    println!();
    read_confirmation("Accept changes?", PromptStream::Stdout)
}
