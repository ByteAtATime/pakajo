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
        for (slot, target) in resolved.iter_mut().zip(targets.iter()) {
            if slot.is_some() {
                continue;
            }
            if let Ok(local) = handle.localdb().pkg(target.as_str()) {
                let mut pkg = Package::from(local);
                let install_date = local.install_date().and_then(|d| (d > 0).then_some(d));
                pkg.installed = Some(InstalledData {
                    version: local.version().to_string(),
                    explicit: matches!(local.reason(), alpm::PackageReason::Explicit),
                    install_date,
                    script: local.has_scriptlet(),
                });
                *slot = Some(pkg);
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
        if !is_local_view(pkg)
            && let Ok(local) = handle.localdb().pkg(pkg.name.as_str())
        {
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

fn local_badge(stdout_color: bool) -> String {
    color::paint(stdout_color, color::YELLOW, "[local]")
}

fn is_local_view(pkg: &Package) -> bool {
    matches!(&pkg.kind, PackageKind::Repo(data) if data.is_local())
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
        PackageKind::Repo(data) if data.is_local() => local_badge(stdout_color),
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
        if data.is_local() {
            rows.push(("Size on Disk", humanized(data.installed_size)));
        } else {
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
        }
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
    use crate::package::{AurData, InstalledData, OptDependency, PackageKind, RepoData};

    fn test_pkg() -> Package {
        Package {
            name: "test-pkg".into(),
            description: Some("Test package description".into()),
            version: "1.0.0-1".into(),
            maintainer: Some("Maintainer <maintainer@archlinux.org>".into()),
            licenses: vec!["MIT".into()],
            groups: vec![],
            provides: vec![],
            conflicts: vec![],
            replaces: vec![],
            dependencies: vec!["dep-a".into(), "dep-b".into()],
            opt_dependencies: vec![],
            upstream_url: Some("https://example.com".into()),
            installed: None,
            kind: PackageKind::Repo(RepoData {
                repo: Some("extra".into()),
                architecture: Some("x86_64".into()),
                installed_size: 2048,
                download_size: 1024,
                build_date: None,
                validated_by: String::new(),
                script: false,
                required_by: vec![],
                optional_for: vec![],
            }),
        }
    }

    #[test]
    fn render_repo_package() {
        let pkg = test_pkg();
        let out = render(&pkg, false);

        assert!(out.starts_with("extra/x86_64 :: test-pkg 1.0.0-1 [not installed]"));
        assert!(out.contains("Test package description"));
        assert!(out.contains("https://example.com"));
        assert!(out.contains("Dependencies (2)"));
        assert!(out.contains("Required        dep-a, dep-b"));
    }

    #[test]
    fn render_aur_package() {
        let mut pkg = test_pkg();
        pkg.maintainer = None;
        pkg.kind = PackageKind::Aur(AurData {
            num_votes: 12500,
            popularity: 82.5,
            submitted: Some(1442236800),
            last_modified: None,
            flagged: Some(1785715200),
            make_depends: vec!["cmake".into()],
            check_depends: vec![],
        });

        let out = render(&pkg, false);

        assert!(out.contains("aur :: test-pkg 1.0.0-1 [aur]"));
        assert!(out.contains("Votes / Pop     12,500 (82.50 popularity)"));
        assert!(out.contains("Maintainer      None (Orphaned)"));
        assert!(out.contains("Flagged Out     Yes"));
        assert!(out.contains("AUR Link        https://aur.archlinux.org/packages/test-pkg"));
        assert!(out.contains("Make Depends    cmake"));
        assert!(out.contains("Runtime Dependencies (2)"));
    }

    #[test]
    fn render_installed_status_variations() {
        let mut pkg = test_pkg();
        pkg.installed = Some(InstalledData {
            version: "0.9.0-1".into(),
            explicit: true,
            install_date: None,
            script: true,
        });

        let out = render(&pkg, false);
        assert!(out.contains("[installed]"));
        assert!(out.contains("\n  Status"));
        assert!(out.contains("(0.9.0-1 installed, explicitly installed)"));
        assert!(out.contains("Install Script  Yes"));

        if let PackageKind::Repo(data) = &mut pkg.kind {
            data.repo = Some("local".into());
        }
        let local_out = render(&pkg, false);
        assert!(local_out.contains("[local]"));
        assert!(local_out.contains("Size on Disk"));
    }

    #[test]
    fn render_optional_dependencies() {
        let mut pkg = test_pkg();
        pkg.opt_dependencies = vec![
            OptDependency {
                name: "python".into(),
                version: Some(">=3.12".into()),
                reason: Some("for scripting".into()),
                installed: true,
            },
            OptDependency {
                name: "ruby".into(),
                version: None,
                reason: None,
                installed: false,
            },
        ];

        let out = render(&pkg, false);
        assert!(out.contains("Optional Dependencies (2)"));
        assert!(out.contains("[✓] python>=3.12              for scripting"));
        assert!(out.contains("[ ] ruby"));
    }

    #[test]
    fn render_truncates_long_lists() {
        let mut pkg = test_pkg();
        pkg.dependencies = (b'a'..=b'h').map(|c| (c as char).to_string()).collect();

        let out = render(&pkg, false);
        assert!(out.contains("Dependencies (8)"));
        assert!(out.contains("Required        a, b, c, d, e, f, ... (+2 more)"));
    }

    #[test]
    fn render_zero_dependencies_with_reverse_deps() {
        let mut pkg = test_pkg();
        pkg.dependencies.clear();
        if let PackageKind::Repo(data) = &mut pkg.kind {
            data.required_by = vec!["meta-package".into()];
        }

        let out = render(&pkg, false);
        assert!(out.contains("Dependencies (0)"));
        assert!(out.contains("Required By     meta-package"));
        assert!(!out.contains("Required        "));
    }

    #[test]
    fn render_omits_missing_fields_cleanly() {
        let mut pkg = test_pkg();
        pkg.description = None;
        pkg.upstream_url = None;
        pkg.dependencies.clear();

        let out = render(&pkg, false);
        assert!(out.contains("extra/x86_64 :: test-pkg 1.0.0-1 [not installed]\n\n  Package Info"));
        assert!(!out.contains("Dependencies"));
        assert!(!out.contains("Validated By"));
    }

    #[test]
    fn render_respects_color_flag() {
        let pkg = test_pkg();
        assert!(!render(&pkg, false).contains('\x1b'));
        assert!(render(&pkg, true).contains("\x1b["));
    }
}
