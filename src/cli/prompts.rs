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

pub(super) fn confirm_install() -> bool {
    print!("\n");
    read_confirmation("Proceed with installation?", PromptStream::Stdout)
}

pub(super) fn confirm_remove() -> bool {
    print!("\n");
    read_confirmation("Proceed with removal?", PromptStream::Stdout)
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
            None => println!(
                "  {:<nw$}  {:<vw$}",
                name,
                v,
                nw = name_width,
                vw = vw,
            ),
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

pub(super) fn confirm_review_accept() -> bool {
    println!();
    read_confirmation("Accept changes?", PromptStream::Stdout)
}
