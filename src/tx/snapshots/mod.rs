mod install;
mod remove;

pub(super) fn snapshot_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"/tmp/\.tmp[a-zA-Z0-9]+", "<tmp>");
    settings.set_snapshot_path(".");
    settings
}

fn approved() -> Box<dyn crate::question::source::AnswerSource> {
    crate::tx::prompt::with_preapproved_proceed(Box::new(crate::question::source::ExploreDefaults))
}

fn installed_names(handle: &alpm::Alpm) -> Vec<String> {
    let mut installed: Vec<String> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|pkg| format!("{} {} {:?}", pkg.name(), pkg.version(), pkg.reason()))
        .collect();
    installed.sort();
    installed
}

fn render_outcome(
    result: &Result<crate::tx::driver::RunOutcome, anyhow::Error>,
    handle: &alpm::Alpm,
) -> String {
    match result {
        Ok(outcome) => {
            let mut rendered = format!("finish: {:?}\n", outcome.finish);
            rendered.push_str("summary:\n");
            for line in summary_lines(outcome) {
                rendered.push_str(&format!("  {line}\n"));
            }
            rendered.push_str("installed:\n");
            for line in installed_names(handle) {
                rendered.push_str(&format!("  {line}\n"));
            }
            rendered
        }
        Err(error) => {
            let mut rendered = format!("error: {error:#}\n");
            rendered.push_str("installed:\n");
            for line in installed_names(handle) {
                rendered.push_str(&format!("  {line}\n"));
            }
            rendered
        }
    }
}

fn summary_lines(outcome: &crate::tx::driver::RunOutcome) -> Vec<String> {
    let mut packages: Vec<String> = outcome
        .summary
        .packages
        .iter()
        .map(|pkg| {
            if pkg.is_removal {
                format!(
                    "{} {} (removal)",
                    pkg.name,
                    pkg.old_version.as_deref().unwrap_or("-"),
                )
            } else {
                format!("{} {}", pkg.name, pkg.new_version)
            }
        })
        .collect();
    packages.sort();
    packages
}
