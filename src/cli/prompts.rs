use std::io::Write as _;

use crate::{color, resolve::BuildPlan};

pub(super) fn confirm_install() -> bool {
    print!("\n");
    confirm_yes("Proceed with installation?")
}

pub(super) fn confirm_remove() -> bool {
    print!("\n");
    confirm_yes("Proceed with removal?")
}

pub(super) fn confirm_build(plan: &BuildPlan) -> bool {
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
    confirm_yes("Proceed with build?")
}

fn confirm_yes(message: &str) -> bool {
    let c = color::stdout_color();
    print!(
        "{} {} ",
        color::colon(c, message),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

pub(super) fn confirm_install_stderr() -> bool {
    let c = color::stderr_color();
    eprint!(
        "\n{} {} ",
        color::colon(c, "Proceed with installation?"),
        color::paint(c, color::BOLD, "[Y/n]")
    );
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "" | "y" | "yes")
}
