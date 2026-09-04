use crate::color;
use crate::package::{InstalledData, OptDependency, Package, PackageKind};

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
            Some(mut pkg) => {
                if let Ok(local) = handle.localdb().pkg(pkg.name.as_str()) {
                    let install_date = local.install_date().and_then(|d| (d > 0).then_some(d));
                    pkg.installed = Some(InstalledData {
                        version: local.version().to_string(),
                        explicit: matches!(local.reason(), alpm::PackageReason::Explicit),
                        install_date,
                        script: local.has_scriptlet(),
                    });
                }
                if !pkg.opt_dependencies.is_empty() {
                    for dep in &mut pkg.opt_dependencies {
                        dep.installed = handle
                            .localdb()
                            .pkgs()
                            .find_satisfier(dep.name.as_str())
                            .is_some();
                    }
                }
                rendered.push(render(&pkg, stdout_color))
            }
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

fn format_date(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .format("%a %d %b %Y")
        .to_string()
}

fn humanized(bytes: i64) -> String {
    let (value, unit) = crate::utils::humanize_size(bytes);
    format!("{value:.2} {unit}")
}

fn installed_row(pkg: &Package) -> Option<(String, String)> {
    let overlay = pkg.installed.as_ref()?;
    let mut note = if overlay.explicit {
        "explicitly installed".to_string()
    } else {
        "installed as a dependency".to_string()
    };
    if overlay.version != pkg.version {
        note = format!("{} installed, {note}", overlay.version);
    }
    let value = match overlay.install_date {
        Some(epoch) => format!("{} ({note})", format_date(epoch)),
        None => format!("({note})"),
    };
    Some(("Installed".to_string(), value))
}

pub fn render(pkg: &Package, stdout_color: bool) -> String {
    let installed = pkg.installed.is_some();
    let mut out = header(pkg, &origin(pkg), installed, stdout_color);
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
    if let Some(rendered) = section(section_title(installed), &package_rows(pkg), stdout_color) {
        out.push_str(&rendered);
    }
    if let PackageKind::Repo(data) = &pkg.kind
        && let Some(rendered) = section(
            &format!("Dependencies ({})", pkg.dependencies.len()),
            &dependency_rows(pkg, data),
            stdout_color,
        )
    {
        out.push_str(&rendered);
    }
    if let Some(rendered) = opt_dependency_section(pkg, stdout_color) {
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

fn header(pkg: &Package, origin: &str, installed: bool, stdout_color: bool) -> String {
    format!(
        "{} {} {} {} {}",
        color::paint(stdout_color, color::COLON, origin),
        color::paint(stdout_color, color::GRAY, "::"),
        color::paint(stdout_color, NAME, &pkg.name),
        color::paint(stdout_color, VERSION, &pkg.version),
        badge(installed, stdout_color),
    )
}

fn push_row(rows: &mut Vec<(String, String)>, label: &str, value: String) {
    rows.push((label.to_string(), value));
}

fn package_rows(pkg: &Package) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(row) = installed_row(pkg) {
        rows.push(row);
    }
    if let PackageKind::Repo(data) = &pkg.kind {
        let script = match pkg.installed.as_ref() {
            Some(overlay) => overlay.script,
            None => data.script,
        };
        push_row(&mut rows, "Install Script", yes_no(script));
        push_row(
            &mut rows,
            "Size",
            format!(
                "{} (download), {} (installed)",
                humanized(data.download_size),
                humanized(data.installed_size),
            ),
        );
        if let Some(epoch) = data.build_date {
            push_row(&mut rows, "Build Date", format_date(epoch));
        }
    }
    push_row(
        &mut rows,
        "Packager",
        pkg.maintainer.clone().unwrap_or_default(),
    );
    push_row(&mut rows, "License", pkg.licenses.join(", "));
    if let PackageKind::Repo(data) = &pkg.kind
        && !data.validated_by.is_empty()
    {
        push_row(&mut rows, "Validated By", data.validated_by.clone());
    }
    push_row(&mut rows, "Groups", pkg.groups.join(", "));
    push_row(&mut rows, "Provides", pkg.provides.join(", "));
    push_row(&mut rows, "Conflicts", pkg.conflicts.join(", "));
    push_row(&mut rows, "Replaces", pkg.replaces.join(", "));
    rows
}

fn yes_no(value: bool) -> String {
    if value {
        "Yes".to_string()
    } else {
        "No".to_string()
    }
}

fn required_value(dependencies: &[String]) -> String {
    if dependencies.is_empty() {
        return String::new();
    }
    if dependencies.len() <= 6 {
        return dependencies.join("  ");
    }
    format!(
        "{}  ... (+{} more)",
        dependencies[..6].join("  "),
        dependencies.len() - 6
    )
}

fn dependency_rows(pkg: &Package, data: &crate::package::RepoData) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    push_row(&mut rows, "Required", required_value(&pkg.dependencies));
    push_row(&mut rows, "Required By", data.required_by.join(", "));
    push_row(&mut rows, "Optional For", data.optional_for.join(", "));
    rows
}

fn opt_dependency_line(dep: &OptDependency, stdout_color: bool) -> String {
    let (check, tint) = match dep.installed {
        true => ("[✓]", color::GREEN),
        false => ("[ ]", color::GRAY),
    };
    let name_tint = match dep.installed {
        true => VALUE,
        false => color::GRAY,
    };
    let mut line = format!(
        "{} {}",
        color::paint(stdout_color, tint, check),
        color::paint(stdout_color, name_tint, &format!("{:<25}", dep.name)),
    );
    if let Some(reason) = dep.reason.as_deref() {
        line.push(' ');
        line.push_str(&color::paint(stdout_color, color::GRAY, reason));
    }
    line
}

fn opt_dependency_section(pkg: &Package, stdout_color: bool) -> Option<String> {
    if pkg.opt_dependencies.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "\n  {}",
        color::paint(
            stdout_color,
            color::COLON,
            &format!("Optional Dependencies ({})", pkg.opt_dependencies.len()),
        )
    ));
    for dep in &pkg.opt_dependencies {
        out.push_str(&format!("\n    {}", opt_dependency_line(dep, stdout_color)));
    }
    Some(out)
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
    use crate::package::{InstalledData, OptDependency, RepoData};

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
            dependencies: vec![
                "cairo".to_string(),
                "glibc".to_string(),
                "libdrm".to_string(),
            ],
            opt_dependencies: vec![],
            upstream_url: Some("https://github.com/hyprwm/Hyprland".to_string()),
            installed: None,
            kind: PackageKind::Repo(RepoData {
                repo: Some("extra".to_string()),
                architecture: Some("x86_64".to_string()),
                installed_size: 0,
                download_size: 0,
                build_date: None,
                validated_by: String::new(),
                script: false,
                required_by: vec!["grimblast-git".to_string(), "hyprpaper".to_string()],
                optional_for: vec!["xdg-desktop-portal-hyprland".to_string()],
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
            installed: None,
            kind: PackageKind::Repo(RepoData {
                repo: Some("core".to_string()),
                architecture: None,
                installed_size: 0,
                download_size: 0,
                build_date: None,
                validated_by: String::new(),
                script: false,
                required_by: Vec::new(),
                optional_for: Vec::new(),
            }),
        }
    }

    #[test]
    fn render_full_plain() {
        let expected = "extra/x86_64 :: hyprland 0.56.2-1 [not installed]\n\
            A highly customizable dynamic tiling Wayland compositor\n\
            https://github.com/hyprwm/Hyprland\n\
            \n  Package Info\n\
            \x20   Install Script  No\n\
            \x20   Size            0.00 B (download), 0.00 B (installed)\n\
            \x20   Packager        Caleb Maclennan <alerque@archlinux.org>\n\
            \x20   License         BSD-3-Clause\n\
            \x20   Groups          hyprland-git-meta, wayland-compositors\n\
            \x20   Provides        wayland-compositor\n\
            \x20   Conflicts       hyprland-git, hyprland-legacy-bin\n\
            \x20   Replaces        hyprland-nvidia\n\
            \n  Dependencies (3)\n\
            \x20   Required        cairo  glibc  libdrm\n\
            \x20   Required By     grimblast-git, hyprpaper\n\
            \x20   Optional For    xdg-desktop-portal-hyprland";
        assert_eq!(render(&full_package(), false), expected);
    }

    #[test]
    fn render_minimal_plain() {
        let expected = "core :: minimal-base 1.0-1 [not installed]\n\
            \n  Package Info\n\
            \x20   Install Script  No\n\
            \x20   Size            0.00 B (download), 0.00 B (installed)";
        assert_eq!(render(&minimal_package(), false), expected);
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

    fn installed_package(epoch: Option<i64>, version: &str) -> Package {
        let mut pkg = full_package();
        if let PackageKind::Repo(data) = &mut pkg.kind {
            data.installed_size = 26214400;
            data.download_size = 8388608;
            data.build_date = Some(1786406400);
            data.validated_by = "SHA-256, Signature".to_string();
            data.script = false;
        }
        pkg.installed = Some(InstalledData {
            version: version.to_string(),
            explicit: true,
            install_date: epoch,
            script: false,
        });
        pkg
    }

    #[test]
    fn render_installed_plain() {
        let epoch = 1786406400;
        let pkg = installed_package(Some(epoch), "0.56.2-1");
        let date = format_date(epoch);
        let expected = format!(
            "extra/x86_64 :: hyprland 0.56.2-1 [installed]\n\
            A highly customizable dynamic tiling Wayland compositor\n\
            https://github.com/hyprwm/Hyprland\n\
            \n  Status\n\
            \x20   Installed       {date} (explicitly installed)\n\
            \x20   Install Script  No\n\
            \x20   Size            8.00 MiB (download), 25.00 MiB (installed)\n\
            \x20   Build Date      {}\n\
            \x20   Packager        Caleb Maclennan <alerque@archlinux.org>\n\
            \x20   License         BSD-3-Clause\n\
            \x20   Validated By    SHA-256, Signature\n\
            \x20   Groups          hyprland-git-meta, wayland-compositors\n\
            \x20   Provides        wayland-compositor\n\
            \x20   Conflicts       hyprland-git, hyprland-legacy-bin\n\
            \x20   Replaces        hyprland-nvidia\n\
            \n  Dependencies (3)\n\
            \x20   Required        cairo  glibc  libdrm\n\
            \x20   Required By     grimblast-git, hyprpaper\n\
            \x20   Optional For    xdg-desktop-portal-hyprland",
            format_date(1786406400),
        );
        assert_eq!(render(&pkg, false), expected);
    }

    #[test]
    fn render_installed_version_diff_note() {
        let pkg = installed_package(None, "0.55.0-1");
        let rendered = render(&pkg, false);
        assert!(rendered.contains("[installed]"));
        assert!(rendered.contains("\n  Status"));
        assert!(rendered.contains("(0.55.0-1 installed, explicitly installed)"));
        let line = rendered
            .lines()
            .find(|line| line.contains("Installed"))
            .expect("Installed row must render");
        assert!(line.starts_with("    Installed      "));
    }

    #[test]
    fn render_validated_by_rows() {
        for (validated_by, expected) in [
            ("Unknown", "Unknown"),
            ("None", "None"),
            ("MD5, SHA-256, Signature", "MD5, SHA-256, Signature"),
        ] {
            let mut pkg = full_package();
            if let PackageKind::Repo(data) = &mut pkg.kind {
                data.validated_by = validated_by.to_string();
            }
            let rendered = render(&pkg, false);
            assert!(
                rendered.contains(&format!("Validated By    {expected}")),
                "validated_by {validated_by} must render; got:\n{rendered}",
            );
        }
        let rendered = render(&full_package(), false);
        assert!(
            !rendered.contains("Validated By"),
            "empty validated_by must skip the row; got:\n{rendered}",
        );
    }

    #[test]
    fn render_installed_row_shape() {
        let pkg = installed_package(Some(1786406400), "0.56.2-1");
        let rendered = render(&pkg, false);
        let line = rendered
            .lines()
            .find(|line| line.contains("Installed"))
            .expect("Installed row must render");
        assert!(line.starts_with("    Installed      "));
        assert!(line.contains("(explicitly installed)"));
        let mut dep = installed_package(None, "0.56.2-1");
        if let Some(overlay) = dep.installed.as_mut() {
            overlay.explicit = false;
        }
        let rendered = render(&dep, false);
        let line = rendered
            .lines()
            .find(|line| line.contains("Installed"))
            .expect("Installed row must render");
        assert!(line.starts_with("    Installed      "));
        assert!(line.contains("(installed as a dependency)"));
    }

    #[test]
    fn format_date_pins_format_tokens() {
        let epoch = 1_754_000_000;
        let expected = chrono::DateTime::from_timestamp(epoch, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%a %d %b %Y")
            .to_string();
        assert_eq!(format_date(epoch), expected);
    }

    #[test]
    fn render_dependencies_truncated_past_six() {
        let mut pkg = full_package();
        pkg.dependencies = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
            "h".to_string(),
        ];
        let rendered = render(&pkg, false);
        assert!(rendered.contains("\n  Dependencies (8)"));
        assert!(rendered.contains("Required        a  b  c  d  e  f  ... (+2 more)"));
    }

    #[test]
    fn render_dependencies_omitted_when_empty() {
        let rendered = render(&minimal_package(), false);
        assert!(!rendered.contains("Dependencies"));
    }

    #[test]
    fn render_dependencies_zero_count_with_reverse_deps() {
        let mut pkg = minimal_package();
        if let PackageKind::Repo(data) = &mut pkg.kind {
            data.required_by = vec!["some-meta".to_string()];
        }
        let rendered = render(&pkg, false);
        assert!(rendered.contains("\n  Dependencies (0)"));
        assert!(rendered.contains("Required By     some-meta"));
        assert!(!rendered.contains("Required        "));
    }

    fn opt_package() -> Package {
        let mut pkg = full_package();
        pkg.opt_dependencies = vec![
            OptDependency {
                name: "cmake".to_string(),
                reason: Some("to build plugins with hyprpm".to_string()),
                installed: true,
            },
            OptDependency {
                name: "hyprshutdown".to_string(),
                reason: Some("clean logout and shutdown helper".to_string()),
                installed: false,
            },
            OptDependency {
                name: "bare-tool".to_string(),
                reason: None,
                installed: false,
            },
        ];
        pkg
    }

    #[test]
    fn render_opt_dependencies_plain() {
        let rendered = render(&opt_package(), false);
        assert!(rendered.contains("\n  Optional Dependencies (3)"));
        assert!(
            rendered.contains("    [✓] cmake                     to build plugins with hyprpm")
        );
        assert!(
            rendered.contains("    [ ] hyprshutdown              clean logout and shutdown helper")
        );
    }

    #[test]
    fn render_opt_dependency_without_reason() {
        let rendered = render(&opt_package(), false);
        let line = rendered
            .lines()
            .find(|line| line.contains("bare-tool"))
            .expect("bare-tool row must render");
        assert_eq!(line, "    [ ] bare-tool                ");
    }

    #[test]
    fn render_opt_dependencies_colored() {
        let rendered = render(&opt_package(), true);
        assert!(rendered.contains("\x1b[1;32m[✓]\x1b[0m"));
        assert!(rendered.contains("\x1b[90m[ ]\x1b[0m"));
        assert!(rendered.contains("\x1b[1;34mOptional Dependencies (3)\x1b[0m"));
    }

    #[test]
    fn render_opt_dependencies_omitted_when_empty() {
        let rendered = render(&full_package(), false);
        assert!(!rendered.contains("Optional Dependencies"));
    }
}
