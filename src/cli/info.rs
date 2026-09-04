use crate::color;
use crate::package::{Package, PackageKind};

const NAME: &str = "\x1b[1;37m";
const VERSION: &str = "\x1b[1;36m";
const VALUE: &str = "\x1b[37m";
const LINK: &str = "\x1b[4;36m";
const NOT_INSTALLED: &str = "\x1b[2;37m";

pub fn run(targets: Vec<String>) -> ! {
    let handle = super::alpm_handle_or_exit();
    let targets = super::dedup_positionals(targets);
    let stdout_color = color::stdout_color();
    let mut rendered: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    for target in &targets {
        match crate::package::find(&handle, target) {
            Some(pkg) => rendered.push(render(&pkg, stdout_color)),
            None => missed.push(target.clone()),
        }
    }
    if !rendered.is_empty() {
        println!("{}", rendered.join("\n\n"));
    }
    if !missed.is_empty() {
        let stderr_color = color::stderr_color();
        for name in &missed {
            eprintln!(
                "{} package '{name}' not found",
                color::paint(stderr_color, color::RED, "error:")
            );
        }
        std::process::exit(1);
    }
    std::process::exit(0);
}

pub fn render(pkg: &Package, stdout_color: bool) -> String {
    let mut out = header(pkg, &origin(pkg), stdout_color);
    if let Some(description) = pkg.description.as_deref()
        && !description.is_empty()
    {
        out.push('\n');
        out.push_str(&color::paint(stdout_color, VALUE, description));
    }
    if let Some(url) = pkg.upstream_url.as_deref()
        && !url.is_empty()
    {
        out.push('\n');
        out.push_str(&color::paint(stdout_color, LINK, url));
    }
    if let Some(rendered) = section(section_title(false), &package_rows(pkg), stdout_color) {
        out.push_str(&rendered);
    }
    out
}

fn origin(pkg: &Package) -> String {
    match &pkg.kind {
        PackageKind::Repo(data) => match (&data.repo, &data.architecture) {
            (Some(repo), Some(architecture)) => format!("{repo}/{architecture}"),
            (Some(repo), None) => repo.clone(),
            (None, Some(architecture)) => architecture.clone(),
            (None, None) => String::new(),
        },
        PackageKind::Aur(_) => "aur".to_string(),
    }
}

fn badge(installed: bool, stdout_color: bool) -> String {
    if installed {
        color::paint(stdout_color, color::GREEN, "[installed]")
    } else {
        color::paint(stdout_color, NOT_INSTALLED, "[not installed]")
    }
}

fn section_title(has_installed: bool) -> &'static str {
    if has_installed {
        "Status"
    } else {
        "Package Info"
    }
}

fn header(pkg: &Package, origin: &str, stdout_color: bool) -> String {
    format!(
        "{} {} {} {} {}",
        color::paint(stdout_color, color::COLON, origin),
        color::paint(stdout_color, color::GRAY, "::"),
        color::paint(stdout_color, NAME, &pkg.name),
        color::paint(stdout_color, VERSION, &pkg.version),
        badge(false, stdout_color),
    )
}

fn package_rows(pkg: &Package) -> Vec<(String, String)> {
    vec![
        (
            "Packager".to_string(),
            pkg.maintainer.clone().unwrap_or_default(),
        ),
        ("License".to_string(), pkg.licenses.join(", ")),
        ("Groups".to_string(), pkg.groups.join(", ")),
        ("Provides".to_string(), pkg.provides.join(", ")),
        ("Conflicts".to_string(), pkg.conflicts.join(", ")),
        ("Replaces".to_string(), pkg.replaces.join(", ")),
    ]
}

fn section(title: &str, rows: &[(String, String)], stdout_color: bool) -> Option<String> {
    let kept: Vec<(&str, &str)> = rows
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
    if kept.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "\n  {}",
        color::paint(stdout_color, color::COLON, title)
    ));
    for (label, value) in kept {
        out.push_str(&format!(
            "\n    {} {}",
            color::paint(stdout_color, color::GRAY, &format!("{label:<15}")),
            color::paint(stdout_color, VALUE, value),
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::RepoData;

    fn full_package() -> Package {
        Package {
            name: "hyprland".to_string(),
            description: Some(
                "A highly customizable dynamic tiling Wayland compositor".to_string(),
            ),
            version: "0.56.2-1".to_string(),
            maintainer: Some("Caleb Maclennan <alerque@archlinux.org>".to_string()),
            licenses: vec!["BSD-3-Clause".to_string()],
            groups: vec![
                "hyprland-git-meta".to_string(),
                "wayland-compositors".to_string(),
            ],
            provides: vec!["wayland-compositor".to_string()],
            conflicts: vec![
                "hyprland-git".to_string(),
                "hyprland-legacy-bin".to_string(),
            ],
            replaces: vec!["hyprland-nvidia".to_string()],
            dependencies: vec![],
            opt_dependencies: vec![],
            upstream_url: Some("https://github.com/hyprwm/Hyprland".to_string()),
            kind: PackageKind::Repo(RepoData {
                repo: Some("extra".to_string()),
                architecture: Some("x86_64".to_string()),
                installed_size: 0,
                download_size: 0,
            }),
        }
    }

    fn minimal_package() -> Package {
        Package {
            name: "minimal-base".to_string(),
            description: None,
            version: "1.0-1".to_string(),
            maintainer: None,
            licenses: vec![],
            groups: vec![],
            provides: vec![],
            conflicts: vec![],
            replaces: vec![],
            dependencies: vec![],
            opt_dependencies: vec![],
            upstream_url: None,
            kind: PackageKind::Repo(RepoData {
                repo: Some("core".to_string()),
                architecture: None,
                installed_size: 0,
                download_size: 0,
            }),
        }
    }

    #[test]
    fn render_full_plain() {
        let expected = "extra/x86_64 :: hyprland 0.56.2-1 [not installed]\n\
            A highly customizable dynamic tiling Wayland compositor\n\
            https://github.com/hyprwm/Hyprland\n\
            \n  Package Info\n\
            \x20   Packager        Caleb Maclennan <alerque@archlinux.org>\n\
            \x20   License         BSD-3-Clause\n\
            \x20   Groups          hyprland-git-meta, wayland-compositors\n\
            \x20   Provides        wayland-compositor\n\
            \x20   Conflicts       hyprland-git, hyprland-legacy-bin\n\
            \x20   Replaces        hyprland-nvidia";
        assert_eq!(render(&full_package(), false), expected);
    }

    #[test]
    fn render_minimal_plain() {
        assert_eq!(
            render(&minimal_package(), false),
            "core :: minimal-base 1.0-1 [not installed]"
        );
    }

    #[test]
    fn render_colored_uses_palette() {
        let rendered = render(&full_package(), true);
        assert!(rendered.contains("\x1b[1;34mextra/x86_64\x1b[0m"));
        assert!(rendered.contains("\x1b[90m::\x1b[0m"));
        assert!(rendered.contains("\x1b[1;37mhyprland\x1b[0m"));
        assert!(rendered.contains("\x1b[1;36m0.56.2-1\x1b[0m"));
        assert!(rendered.contains("\x1b[2;37m[not installed]\x1b[0m"));
        assert!(
            rendered
                .contains("\x1b[37mA highly customizable dynamic tiling Wayland compositor\x1b[0m")
        );
        assert!(rendered.contains("\x1b[4;36mhttps://github.com/hyprwm/Hyprland\x1b[0m"));
        assert!(rendered.contains("\x1b[1;34mPackage Info\x1b[0m"));
        assert!(rendered.contains("\x1b[90mPackager       \x1b[0m"));
        assert!(rendered.contains("\x1b[37mBSD-3-Clause\x1b[0m"));
    }
}
