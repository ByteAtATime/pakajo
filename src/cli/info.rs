use crate::color;
use crate::package::{InstalledData, OptDependency, Package, PackageKind};

const NAME: &str = "\x1b[1;37m";
const VERSION: &str = "\x1b[1;36m";
const VALUE: &str = "\x1b[37m";
const LINK: &str = "\x1b[4;36m";
const NOT_INSTALLED: &str = "\x1b[2;37m";
const DEP_PREVIEW: usize = 6;
const AUR_DEP_PREVIEW: usize = 4;

pub fn run(targets: Vec<String>) -> ! {
    let handle = super::alpm_handle_or_exit();
    let targets = super::dedup_positionals(targets);
    let stdout_color = color::stdout_color();
    let mut resolved: Vec<Option<Package>> = Vec::with_capacity(targets.len());
    let mut pending: Vec<String> = Vec::new();
    for target in &targets {
        match crate::package::find(&handle, target) {
            Some(pkg) => resolved.push(Some(pkg)),
            None => {
                resolved.push(None);
                pending.push(target.clone());
            }
        }
    }
    if !pending.is_empty() {
        let aur = resolve_aur(&pending);
        for (slot, target) in resolved.iter_mut().zip(targets.iter()) {
            if slot.is_none()
                && let Some(info) = aur.get(target)
            {
                *slot = Some(Package::from(info.clone()));
            }
        }
    }
    let mut rendered: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    for (target, slot) in targets.iter().zip(resolved.iter_mut()) {
        let Some(pkg) = slot else {
            missed.push(target.clone());
            continue;
        };
        if let Ok(local) = handle.localdb().pkg(pkg.name.as_str()) {
            let install_date = local.install_date().and_then(|d| (d > 0).then_some(d));
            pkg.installed = Some(InstalledData {
                version: local.version().to_string(),
                explicit: matches!(local.reason(), alpm::PackageReason::Explicit),
                install_date,
                script: local.has_scriptlet(),
            });
        }
        let pkgs = handle.localdb().pkgs();
        for dep in &mut pkg.opt_dependencies {
            let constraint = dep.version.as_deref().unwrap_or_default();
            dep.installed = pkgs
                .find_satisfier(format!("{}{constraint}", dep.name))
                .is_some();
        }
        rendered.push(render(pkg, stdout_color))
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

fn resolve_aur(names: &[String]) -> std::collections::HashMap<String, crate::aur::AurInfo> {
    let client = crate::aur::AurClient::new();
    match client.info_many(names) {
        Ok(infos) => infos
            .into_iter()
            .map(|info| (info.name.clone(), info))
            .collect(),
        Err(_) => cached_aur(names),
    }
}

fn cached_aur(names: &[String]) -> std::collections::HashMap<String, crate::aur::AurInfo> {
    let path = match crate::db::PackageDb::db_path() {
        Ok(path) => path,
        Err(_) => return std::collections::HashMap::new(),
    };
    let db = match crate::db::PackageDb::open(&path) {
        Ok(db) => db,
        Err(_) => return std::collections::HashMap::new(),
    };
    let mut found = std::collections::HashMap::new();
    for name in names {
        if let Ok(Some(info)) = db.detail(name) {
            found.insert(name.clone(), info);
        }
    }
    found
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

fn format_ymd(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string()
}

fn grouped_votes(votes: u64) -> String {
    let digits: Vec<char> = votes.to_string().chars().collect();
    let mut out = String::new();
    for (index, digit) in digits.iter().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(*digit);
    }
    out
}

fn installed_row(pkg: &Package, stdout_color: bool) -> Option<(&'static str, String)> {
    let overlay = pkg.installed.as_ref()?;
    let base: &str = if overlay.explicit {
        "explicitly installed"
    } else {
        "installed as dependency"
    };
    let note = if overlay.version != pkg.version {
        format!("{} installed, {base}", overlay.version)
    } else {
        base.to_string()
    };
    let fragment = color::paint(stdout_color, color::GRAY, &format!("({note})"));
    let value = match overlay.install_date {
        Some(epoch) => format!("{} {fragment}", format_date(epoch)),
        None => fragment,
    };
    Some(("Installed", value))
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
    if let Some(rendered) = section(
        section_title(installed),
        &package_rows(pkg, stdout_color),
        stdout_color,
    ) {
        out.push_str(&rendered);
    }
    if let PackageKind::Aur(data) = &pkg.kind {
        if let Some(rendered) = section(
            "Community & Maintenance",
            &community_rows(pkg, data, stdout_color),
            stdout_color,
        ) {
            out.push_str(&rendered);
        }
        if let Some(rendered) = build_source_section(pkg, data, stdout_color) {
            out.push_str(&rendered);
        }
        if let Some(rendered) = section(
            &format!("Runtime Dependencies ({})", pkg.dependencies.len()),
            &aur_runtime_rows(pkg, stdout_color),
            stdout_color,
        ) {
            out.push_str(&rendered);
        }
    }
    if let PackageKind::Repo(data) = &pkg.kind
        && let Some(rendered) = section(
            &format!("Dependencies ({})", pkg.dependencies.len()),
            &dependency_rows(pkg, data, stdout_color),
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

fn aur_badge(stdout_color: bool) -> String {
    color::paint(stdout_color, color::MAGENTA, "[aur]")
}

fn section_title(has_installed: bool) -> &'static str {
    if has_installed {
        "Status"
    } else {
        "Package Info"
    }
}

fn header(pkg: &Package, origin: &str, installed: bool, stdout_color: bool) -> String {
    let badges = match &pkg.kind {
        PackageKind::Aur(_) if installed => {
            format!("{} {}", aur_badge(stdout_color), badge(true, stdout_color))
        }
        PackageKind::Aur(_) => aur_badge(stdout_color),
        PackageKind::Repo(_) => badge(installed, stdout_color),
    };
    format!(
        "{} {} {} {} {}",
        color::paint(stdout_color, color::COLON, origin),
        color::paint(stdout_color, color::GRAY, "::"),
        color::paint(stdout_color, NAME, &pkg.name),
        color::paint(stdout_color, VERSION, &pkg.version),
        badges,
    )
}

fn package_rows(pkg: &Package, stdout_color: bool) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(row) = installed_row(pkg, stdout_color) {
        rows.push(row);
    }
    if let PackageKind::Repo(data) = &pkg.kind {
        let script = match pkg.installed.as_ref() {
            Some(overlay) => overlay.script,
            None => data.script,
        };
        rows.push(("Install Script", yes_no(script)));
        rows.push((
            "Size",
            format!(
                "{} {}, {} {}",
                humanized(data.download_size),
                color::paint(stdout_color, color::GRAY, "(download)"),
                humanized(data.installed_size),
                color::paint(stdout_color, color::GRAY, "(installed)"),
            ),
        ));
        if let Some(epoch) = data.build_date {
            rows.push(("Build Date", format_date(epoch)));
        }
    }
    if matches!(pkg.kind, PackageKind::Repo(_)) {
        rows.push(("Packager", pkg.maintainer.clone().unwrap_or_default()));
    }
    if matches!(pkg.kind, PackageKind::Repo(_)) {
        rows.push(("License", pkg.licenses.join(", ")));
    }
    if let PackageKind::Repo(data) = &pkg.kind
        && !data.validated_by.is_empty()
    {
        rows.push(("Validated By", data.validated_by.clone()));
    }
    if matches!(pkg.kind, PackageKind::Repo(_)) {
        rows.push(("Groups", pkg.groups.join(", ")));
        rows.push(("Provides", pkg.provides.join(", ")));
        rows.push(("Conflicts", pkg.conflicts.join(", ")));
        rows.push(("Replaces", pkg.replaces.join(", ")));
    }
    rows
}

fn community_rows(
    pkg: &Package,
    data: &crate::package::AurData,
    stdout_color: bool,
) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    rows.push((
        "Votes / Pop",
        format!(
            "{} {}",
            grouped_votes(data.num_votes),
            color::paint(
                stdout_color,
                color::GRAY,
                &format!("({:.2} popularity)", data.popularity)
            ),
        ),
    ));
    match pkg.maintainer.as_deref().filter(|name| !name.is_empty()) {
        Some(name) => rows.push(("Maintainer", name.to_string())),
        None => rows.push((
            "Maintainer",
            color::paint(stdout_color, color::YELLOW, "None (Orphaned)"),
        )),
    }
    if let Some(epoch) = data.submitted {
        rows.push(("Submitted", format_date(epoch)));
    }
    if let Some(epoch) = data.last_modified {
        rows.push(("Last Modified", format_date(epoch)));
    }
    match data.flagged {
        Some(epoch) => rows.push((
            "Flagged Out",
            color::paint(
                stdout_color,
                color::YELLOW,
                &format!("Yes ({})", format_ymd(epoch)),
            ),
        )),
        None => rows.push(("Flagged Out", "No".to_string())),
    }
    rows
}

fn aur_link(name: &str) -> String {
    format!("https://aur.archlinux.org/packages/{name}")
}

fn build_source_section(
    pkg: &Package,
    data: &crate::package::AurData,
    stdout_color: bool,
) -> Option<String> {
    let rows: Vec<(&'static str, String)> = vec![
        ("AUR Link", aur_link(&pkg.name)),
        ("License", pkg.licenses.join(", ")),
        (
            "Make Depends",
            truncated_join(&data.make_depends, AUR_DEP_PREVIEW, stdout_color),
        ),
        (
            "Check Depends",
            truncated_join(&data.check_depends, AUR_DEP_PREVIEW, stdout_color),
        ),
        ("Provides", pkg.provides.join(", ")),
        ("Conflicts", pkg.conflicts.join(", ")),
        ("Replaces", pkg.replaces.join(", ")),
    ];
    let kept: Vec<_> = rows.iter().filter(|(_, value)| !value.is_empty()).collect();
    if kept.is_empty() {
        return None;
    }
    let mut out = section_title_line("Build & Source", stdout_color);
    for (label, value) in kept {
        let tint = if *label == "AUR Link" { LINK } else { VALUE };
        out.push_str(&format!(
            "\n    {} {}",
            color::paint(stdout_color, color::GRAY, &format!("{label:<15}")),
            color::paint(stdout_color, tint, value),
        ));
    }
    Some(out)
}

fn aur_runtime_rows(pkg: &Package, stdout_color: bool) -> Vec<(&'static str, String)> {
    vec![(
        "Required",
        truncated_join(&pkg.dependencies, AUR_DEP_PREVIEW, stdout_color),
    )]
}

fn yes_no(value: bool) -> String {
    if value {
        "Yes".to_string()
    } else {
        "No".to_string()
    }
}

fn truncated_join(names: &[String], limit: usize, stdout_color: bool) -> String {
    if names.len() <= limit {
        return names.join(", ");
    }
    format!(
        "{}, {}",
        names[..limit].join(", "),
        color::paint(
            stdout_color,
            color::GRAY,
            &format!("... (+{} more)", names.len() - limit)
        )
    )
}

fn dependency_rows(
    pkg: &Package,
    data: &crate::package::RepoData,
    stdout_color: bool,
) -> Vec<(&'static str, String)> {
    vec![
        (
            "Required",
            truncated_join(&pkg.dependencies, DEP_PREVIEW, stdout_color),
        ),
        (
            "Required By",
            truncated_join(&data.required_by, DEP_PREVIEW, stdout_color),
        ),
        (
            "Optional For",
            truncated_join(&data.optional_for, DEP_PREVIEW, stdout_color),
        ),
    ]
}

fn opt_dependency_line(dep: &OptDependency, stdout_color: bool) -> String {
    let (check, tint, name_tint) = match dep.installed {
        true => ("[✓]", color::GREEN, VALUE),
        false => ("[ ]", color::GRAY, color::GRAY),
    };
    let constraint = dep.version.as_deref().unwrap_or_default();
    let qualified = format!("{}{constraint}", dep.name);
    let mut line = format!(
        "{} {}",
        color::paint(stdout_color, tint, check),
        color::paint(stdout_color, name_tint, &format!("{qualified:<25}")),
    );
    if let Some(reason) = dep.reason.as_deref() {
        line.push(' ');
        line.push_str(&color::paint(stdout_color, color::GRAY, reason));
    }
    line
}

fn section_title_line(title: &str, stdout_color: bool) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "\n  {}",
        color::paint(stdout_color, color::COLON, title)
    ));
    out
}

fn opt_dependency_section(pkg: &Package, stdout_color: bool) -> Option<String> {
    if pkg.opt_dependencies.is_empty() {
        return None;
    }
    let mut out = section_title_line(
        &format!("Optional Dependencies ({})", pkg.opt_dependencies.len()),
        stdout_color,
    );
    for dep in &pkg.opt_dependencies {
        out.push_str(&format!("\n    {}", opt_dependency_line(dep, stdout_color)));
    }
    Some(out)
}

fn section(title: &str, rows: &[(&'static str, String)], stdout_color: bool) -> Option<String> {
    let kept: Vec<_> = rows.iter().filter(|(_, value)| !value.is_empty()).collect();
    if kept.is_empty() {
        return None;
    }
    let mut out = section_title_line(title, stdout_color);
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
    use crate::package::{AurData, InstalledData, OptDependency, RepoData};

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
            \x20   Required        cairo, glibc, libdrm\n\
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
            \x20   Build Date      {date}\n\
            \x20   Packager        Caleb Maclennan <alerque@archlinux.org>\n\
            \x20   License         BSD-3-Clause\n\
            \x20   Validated By    SHA-256, Signature\n\
            \x20   Groups          hyprland-git-meta, wayland-compositors\n\
            \x20   Provides        wayland-compositor\n\
            \x20   Conflicts       hyprland-git, hyprland-legacy-bin\n\
            \x20   Replaces        hyprland-nvidia\n\
            \n  Dependencies (3)\n\
            \x20   Required        cairo, glibc, libdrm\n\
            \x20   Required By     grimblast-git, hyprpaper\n\
            \x20   Optional For    xdg-desktop-portal-hyprland",
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
        assert!(line.contains("(installed as dependency)"));
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
        assert!(rendered.contains("Required        a, b, c, d, e, f, ... (+2 more)"));
    }

    #[test]
    fn render_reverse_dependencies_truncated_past_six() {
        let mut pkg = full_package();
        if let PackageKind::Repo(data) = &mut pkg.kind {
            data.required_by = vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "f".to_string(),
                "g".to_string(),
            ];
            data.optional_for = vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "f".to_string(),
                "g".to_string(),
                "h".to_string(),
            ];
        }
        let rendered = render(&pkg, false);
        assert!(rendered.contains("Required By     a, b, c, d, e, f, ... (+1 more)"));
        assert!(rendered.contains("Optional For    a, b, c, d, e, f, ... (+2 more)"));
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

    fn opt_dep(name: &str, reason: Option<&str>, installed: bool) -> OptDependency {
        OptDependency {
            name: name.to_string(),
            version: None,
            reason: reason.map(|r| r.to_string()),
            installed,
        }
    }

    fn opt_package() -> Package {
        let mut pkg = full_package();
        pkg.opt_dependencies = vec![
            opt_dep("cmake", Some("to build plugins with hyprpm"), true),
            opt_dep(
                "hyprshutdown",
                Some("clean logout and shutdown helper"),
                false,
            ),
            opt_dep("bare-tool", None, false),
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
        assert!(rendered.contains(&format!("\x1b[37m{:<25}\x1b[0m", "cmake")));
        assert!(rendered.contains(&format!("\x1b[90m{:<25}\x1b[0m", "hyprshutdown")));
    }

    #[test]
    fn render_opt_dependency_version_constraint() {
        let mut pkg = full_package();
        let mut dep = opt_dep("python", Some("some reason"), true);
        dep.version = Some(">=3.12".to_string());
        pkg.opt_dependencies = vec![dep];
        let rendered = render(&pkg, false);
        let line = rendered
            .lines()
            .find(|line| line.contains("python"))
            .expect("versioned row must render");
        assert!(line.contains("python>=3.12"));
    }

    #[test]
    fn render_opt_dependency_long_name_overflow() {
        let mut pkg = full_package();
        pkg.opt_dependencies = vec![opt_dep(
            "a-very-long-optional-dependency-name",
            Some("some reason"),
            false,
        )];
        let rendered = render(&pkg, false);
        let line = rendered
            .lines()
            .find(|line| line.contains("a-very-long-optional-dependency-name"))
            .expect("long name row must render");
        assert_eq!(
            line,
            "    [ ] a-very-long-optional-dependency-name some reason"
        );
    }

    #[test]
    fn render_opt_dependencies_omitted_when_empty() {
        let rendered = render(&full_package(), false);
        assert!(!rendered.contains("Optional Dependencies"));
    }

    fn aur_package() -> Package {
        Package {
            name: "visual-studio-code-bin".to_string(),
            description: Some(
                "Visual Studio Code (vscode): Editor for building and debugging".to_string(),
            ),
            version: "1.93.1-1".to_string(),
            maintainer: Some("dcelasun".to_string()),
            licenses: vec!["custom:commercial".to_string()],
            groups: vec![],
            provides: vec![],
            conflicts: vec![],
            replaces: vec![],
            dependencies: vec![],
            opt_dependencies: vec![],
            upstream_url: Some("https://code.visualstudio.com/".to_string()),
            installed: None,
            kind: PackageKind::Aur(AurData {
                num_votes: 2841,
                popularity: 48.12,
                submitted: Some(1442236800),
                last_modified: Some(1785360000),
                flagged: Some(1785715200),
                make_depends: Vec::new(),
                check_depends: Vec::new(),
            }),
        }
    }

    #[test]
    fn render_aur_plain() {
        let pkg = aur_package();
        let submitted = format_date(1442236800);
        let modified = format_date(1785360000);
        let flagged = format_ymd(1785715200);
        let expected = format!(
            "aur :: visual-studio-code-bin 1.93.1-1 [aur]\n\
            Visual Studio Code (vscode): Editor for building and debugging\n\
            https://code.visualstudio.com/\n\
            \n  Community & Maintenance\n\
            \x20   Votes / Pop     2,841 (48.12 popularity)\n\
            \x20   Maintainer      dcelasun\n\
            \x20   Submitted       {submitted}\n\
            \x20   Last Modified   {modified}\n\
            \x20   Flagged Out     Yes ({flagged})\n\
            \n  Build & Source\n\
            \x20   AUR Link        https://aur.archlinux.org/packages/visual-studio-code-bin\n\
            \x20   License         custom:commercial",
        );
        assert_eq!(render(&pkg, false), expected);
    }

    #[test]
    fn render_aur_installed_plain() {
        let epoch = 1786406400;
        let mut pkg = aur_package();
        pkg.installed = Some(InstalledData {
            version: "1.93.1-1".to_string(),
            explicit: true,
            install_date: Some(epoch),
            script: false,
        });
        let date = format_date(epoch);
        let rendered = render(&pkg, false);
        assert!(rendered.contains("aur :: visual-studio-code-bin 1.93.1-1 [aur] [installed]"));
        assert!(rendered.contains("\n  Status"));
        assert!(rendered.contains(&format!("Installed       {date} (explicitly installed)")));
        assert!(rendered.contains("\n  Community & Maintenance"));
    }

    #[test]
    fn render_aur_colored_fragments_byte_exact() {
        let rendered = render(&aur_package(), true);
        assert!(rendered.contains("\x1b[1;35m[aur]\x1b[0m"));
        assert!(rendered.contains("\x1b[1;34maur\x1b[0m"));
        assert!(rendered.contains("\x1b[37m2,841 \x1b[90m(48.12 popularity)\x1b[0m\x1b[0m"));
        assert!(rendered.contains("\x1b[37mdcelasun\x1b[0m"));
        let flagged = format_ymd(1785715200);
        assert!(rendered.contains(&format!("\x1b[1;33mYes ({flagged})\x1b[0m")));
    }

    #[test]
    fn render_aur_orphan_maintainer() {
        for maintainer in [None, Some(String::new())] {
            let mut pkg = aur_package();
            pkg.maintainer = maintainer;
            let plain = render(&pkg, false);
            assert!(
                plain.contains("Maintainer      None (Orphaned)"),
                "orphan must render plain fallback; got:\n{plain}",
            );
            let colored = render(&pkg, true);
            assert!(
                colored.contains("\x1b[1;33mNone (Orphaned)\x1b[0m"),
                "orphan must warn in yellow; got:\n{colored}",
            );
        }
    }

    #[test]
    fn render_aur_epoch_zero_omits_date_rows() {
        let info = crate::aur::AurInfo {
            id: 1,
            name: "orphan-toy".to_string(),
            package_base_id: 2,
            package_base: "orphan-toy".to_string(),
            version: "0.0.1-1".to_string(),
            description: None,
            url: None,
            num_votes: 0,
            popularity: 0.0,
            out_of_date: None,
            maintainer: None,
            first_submitted: 0,
            last_modified: 0,
            url_path: None,
            submitter: None,
            depends: vec![],
            make_depends: vec![],
            check_depends: vec![],
            opt_depends: vec![],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
            groups: vec![],
            license: vec![],
            keywords: vec![],
            co_maintainers: vec![],
        };
        let pkg = Package::from(info);
        let PackageKind::Aur(data) = &pkg.kind else {
            panic!("expected aur package kind");
        };
        assert_eq!(data.submitted, None);
        assert_eq!(data.last_modified, None);
        assert_eq!(data.flagged, None);
        let rendered = render(&pkg, false);
        assert!(!rendered.contains("Submitted"));
        assert!(!rendered.contains("Last Modified"));
        assert!(rendered.contains("Flagged Out     No"));
        assert!(!rendered.contains("Package Info"));
        assert!(!rendered.contains("Dependencies"));
    }

    #[test]
    fn render_aur_skips_repo_rows() {
        let rendered = render(&aur_package(), false);
        for row in [
            "Install Script",
            "Packager",
            "Validated By",
            "Groups",
            "Dependencies",
        ] {
            assert!(
                !rendered.contains(row),
                "aur view must skip {row}; got:\n{rendered}",
            );
        }
    }

    fn rich_aur_package() -> Package {
        let mut pkg = aur_package();
        if let PackageKind::Aur(data) = &mut pkg.kind {
            data.make_depends = vec![
                "git".to_string(),
                "cmake".to_string(),
                "ninja".to_string(),
                "python".to_string(),
                "go".to_string(),
                "rust".to_string(),
            ];
            data.check_depends = vec![
                "xvfb-run".to_string(),
                "pytest".to_string(),
                "gtest".to_string(),
                "valgrind".to_string(),
                "clang".to_string(),
            ];
        }
        pkg.provides = vec!["code".to_string(), "vscode".to_string()];
        pkg.conflicts = vec!["code".to_string(), "vscode".to_string()];
        pkg.replaces = vec!["visual-studio-code".to_string()];
        pkg.dependencies = vec![
            "alsa-lib".to_string(),
            "gtk3".to_string(),
            "libsecret".to_string(),
            "nss".to_string(),
            "libx11".to_string(),
            "libxkbfile".to_string(),
        ];
        pkg
    }

    #[test]
    fn render_aur_build_source_plain() {
        let rendered = render(&rich_aur_package(), false);
        assert!(rendered.contains("\n  Build & Source"));
        assert!(
            rendered.contains(
                "AUR Link        https://aur.archlinux.org/packages/visual-studio-code-bin"
            )
        );
        assert!(rendered.contains("License         custom:commercial"));
        assert!(rendered.contains("Make Depends    git, cmake, ninja, python, ... (+2 more)"));
        assert!(
            rendered.contains("Check Depends   xvfb-run, pytest, gtest, valgrind, ... (+1 more)")
        );
        assert!(rendered.contains("Provides        code, vscode"));
        assert!(rendered.contains("Conflicts       code, vscode"));
        assert!(rendered.contains("Replaces        visual-studio-code"));
    }

    #[test]
    fn render_aur_build_source_link_colored() {
        let rendered = render(&rich_aur_package(), true);
        assert!(rendered.contains(
            "\x1b[4;36mhttps://aur.archlinux.org/packages/visual-studio-code-bin\x1b[0m"
        ));
        assert!(rendered.contains("\x1b[37mcustom:commercial\x1b[0m"));
        assert!(
            rendered
                .contains("\x1b[37mgit, cmake, ninja, python, \x1b[90m... (+2 more)\x1b[0m\x1b[0m")
        );
    }

    #[test]
    fn render_aur_runtime_dependencies_truncated_at_four() {
        let rendered = render(&rich_aur_package(), false);
        assert!(rendered.contains("\n  Runtime Dependencies (6)"));
        assert!(rendered.contains("Required        alsa-lib, gtk3, libsecret, nss, ... (+2 more)"));
    }

    #[test]
    fn render_aur_opt_dependencies_plain() {
        let mut pkg = rich_aur_package();
        pkg.opt_dependencies = vec![
            opt_dep("bash-completion", Some("bash completion support"), true),
            opt_dep(
                "org.freedesktop.secrets",
                Some("keyring credential storage"),
                false,
            ),
            opt_dep("zsh-completions", Some("zsh completion support"), true),
        ];
        let rendered = render(&pkg, false);
        assert!(rendered.contains("\n  Optional Dependencies (3)"));
        assert!(rendered.contains("    [✓] bash-completion           bash completion support"));
        assert!(rendered.contains("    [ ] org.freedesktop.secrets   keyring credential storage"));
        assert!(rendered.contains("    [✓] zsh-completions           zsh completion support"));
        let runtime = rendered
            .find("Runtime Dependencies (6)")
            .expect("runtime must render");
        let optional = rendered
            .find("Optional Dependencies (3)")
            .expect("optional must render");
        assert!(runtime < optional);
    }

    #[test]
    fn render_aur_opt_dependencies_colored() {
        let mut pkg = aur_package();
        pkg.opt_dependencies = vec![
            opt_dep("bash-completion", Some("bash completion support"), true),
            opt_dep(
                "org.freedesktop.secrets",
                Some("keyring credential storage"),
                false,
            ),
        ];
        let rendered = render(&pkg, true);
        assert!(rendered.contains("\x1b[1;34mOptional Dependencies (2)\x1b[0m"));
        assert!(rendered.contains("\x1b[1;32m[✓]\x1b[0m"));
        assert!(rendered.contains("\x1b[90m[ ]\x1b[0m"));
        assert!(rendered.contains(&format!("\x1b[37m{:<25}\x1b[0m", "bash-completion")));
        assert!(rendered.contains(&format!("\x1b[90m{:<25}\x1b[0m", "org.freedesktop.secrets")));
    }

    #[test]
    fn render_aur_license_moved_from_leading_section() {
        let rendered = render(&aur_package(), false);
        assert!(
            !rendered.contains("Package Info"),
            "not-installed aur must have no leading section; got:\n{rendered}",
        );
        let community = rendered
            .find("Community & Maintenance")
            .expect("community must render");
        let build = rendered.find("Build & Source").expect("build must render");
        assert!(community < build);
        let license = rendered.find("License").expect("license must render");
        assert!(license > build);
    }

    #[test]
    fn grouped_votes_separator_cases() {
        for (votes, expected) in [
            (0, "0"),
            (12, "12"),
            (1234, "1,234"),
            (2841, "2,841"),
            (1000000, "1,000,000"),
        ] {
            assert_eq!(grouped_votes(votes), expected);
        }
    }

    #[test]
    fn render_colored_fragments_byte_exact() {
        let epoch = 1786406400;
        let mut pkg = installed_package(Some(epoch), "0.56.2-1");
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
        let date = format_date(epoch);
        let rendered = render(&pkg, true);
        assert!(rendered.contains(&format!(
            "\x1b[37m{date} \x1b[90m(explicitly installed)\x1b[0m\x1b[0m"
        )));
        assert!(rendered.contains(
            "\x1b[37m8.00 MiB \x1b[90m(download)\x1b[0m, 25.00 MiB \x1b[90m(installed)\x1b[0m\x1b[0m"
        ));
        assert!(rendered.contains("\x1b[37ma, b, c, d, e, f, \x1b[90m... (+2 more)\x1b[0m\x1b[0m"));
    }
}
