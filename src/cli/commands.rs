use anyhow::Context as _;
use base64::Engine as _;

use super::classify_target;
use super::privs::stdin_is_tty;
use super::prompts::{confirm_install, confirm_install_stderr};
use super::sinks::{ConsoleSink, EscalatedSink, JsonSink};
use crate::install::{self, InstallTarget};
use crate::package::PackageSource;
use crate::search::SearchResult;

pub fn alpm_handle() -> anyhow::Result<alpm::Alpm> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    crate::pacman::init_alpm(&config)
}

pub fn run_aur_sync() -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let index = crate::package_db::PackageDb::open(&crate::package_db::PackageDb::db_path()?)?;
    match index.refresh(&handle)? {
        crate::package_db::RefreshOutcome::NotModified => println!("index up to date"),
        crate::package_db::RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        } => {
            println!("indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})");
        }
    }
    Ok(())
}

pub fn run_gendb() -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let arch = handle
        .architectures()
        .first()
        .context("no architecture configured in alpm")?;

    let sync_names: std::collections::HashSet<String> = handle
        .syncdbs()
        .iter()
        .flat_map(|db| db.pkgs().iter())
        .map(|p| p.name().to_string())
        .collect();
    let foreign: Vec<String> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|p| p.name().to_string())
        .filter(|name| !sync_names.contains(name))
        .collect();

    let mut devel = crate::devel::load_devel_info();

    if foreign.is_empty() {
        crate::devel::save_devel_info(&devel)?;
        println!(
            "no foreign packages installed; wrote {}",
            crate::devel::state_path().display()
        );
        return Ok(());
    }

    let aur = crate::aur::AurClient::new();
    let infos = match aur.info_many(&foreign) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("warning: AUR info lookup failed: {e:#}");
            crate::devel::save_devel_info(&devel)?;
            return Ok(());
        }
    };

    let mut base_to_names: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for info in &infos {
        base_to_names
            .entry(info.package_base.clone())
            .or_default()
            .push(info.name.clone());
    }

    let mut recorded = 0usize;
    for (base, names) in &base_to_names {
        let pkg_info = match fetch_base_devel_info(base, arch) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("warning: skipping {base}: {e:#}");
                continue;
            }
        };
        for name in names {
            devel.info.insert(name.clone(), pkg_info.clone());
        }
        recorded += 1;
    }

    let path = crate::devel::state_path();
    crate::devel::save_devel_info(&devel)?;
    println!("recorded {recorded} devel package(s) to {}", path.display());
    Ok(())
}

fn fetch_base_devel_info(base: &str, arch: &str) -> anyhow::Result<Option<crate::devel::PkgInfo>> {
    let dir = crate::build::clone_dir(base)?;
    crate::build::git_clone_or_pull(&dir, base)?;
    let srcinfo = if dir.join(".SRCINFO").exists() {
        crate::srcinfo_io::read_from_dir(&dir)?
    } else {
        crate::srcinfo_io::generate(&dir)?
    };
    let pkg_info = crate::devel::fetch_devel_info(arch, &srcinfo)?;
    if pkg_info.repos.is_empty() {
        return Ok(None);
    }
    Ok(Some(pkg_info))
}

pub fn run_search(query: &str) -> anyhow::Result<()> {
    let sqlite_path = crate::package_db::PackageDb::db_path()?;
    let local = crate::package_db::PackageDb::open(&sqlite_path)?;
    let engine = crate::search::engine::SearchEngine::new(sqlite_path)?;
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let snapshot = crate::pacman::snapshot::get(&config)?;
    let installed: std::collections::HashSet<String> = snapshot.installed.into_iter().collect();
    let results = crate::search::dispatch_search(
        &engine,
        &local,
        &installed,
        query,
        &snapshot.groups,
    );
    print_search_results(&results);
    Ok(())
}

fn print_search_results(rows: &[SearchResult]) {
    if rows.is_empty() {
        return;
    }
    let color = crate::color::stdout_color();
    for row in rows {
        let repo_raw = row.repo.as_deref().unwrap_or("-");
        let repo_color = match row.source {
            PackageSource::Repo => crate::color::COLON,
            PackageSource::Aur => crate::color::MAGENTA,
            PackageSource::Group => crate::color::CYAN,
        };
        let repo = crate::color::paint(color, repo_color, repo_raw);
        let name = crate::color::paint(color, crate::color::BOLD, &row.name);
        let version = crate::color::paint(color, crate::color::GREEN, &row.version);

        let mut tokens: Vec<String> = Vec::new();
        if let Some(v) = row.num_votes {
            let code = if v >= 10 {
                crate::color::GREEN
            } else {
                crate::color::GRAY
            };
            tokens.push(crate::color::paint(color, code, &format!("+{v}")));
            if let Some(p) = row.popularity {
                tokens.push(crate::color::paint(color, code, &format!("~{p:.2}")));
            }
        }
        if row.installed {
            tokens.push(crate::color::paint(
                color,
                crate::color::GREEN,
                "[installed]",
            ));
        }

        let metadata_block = if tokens.is_empty() {
            String::new()
        } else {
            let bullet = crate::color::paint(color, crate::color::DIM, "\u{2022}");
            format!(" {bullet} {}", tokens.join(" "))
        };

        let desc = row.description.as_deref().unwrap_or("-");
        println!("{repo}/{name} {version}{metadata_block}");
        println!("    {desc}");
    }
}

pub fn decode_approvals(b64: &str) -> anyhow::Result<crate::question::Approvals> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("--approvals is not valid base64")?;
    serde_json::from_slice(&bytes).context("--approvals is not valid JSON")
}

pub fn answerer_for(
    approvals: Option<crate::question::Approvals>,
) -> Box<dyn crate::answerer::QuestionAnswerer> {
    if let Some(appr) = approvals {
        Box::new(crate::answerer::ApprovalsAnswerer::new(appr))
    } else if stdin_is_tty() {
        Box::new(crate::answerer::StdioAnswerer::new())
    } else {
        Box::new(crate::answerer::NonInteractiveAnswerer)
    }
}

pub fn root_install(
    handle: &alpm::Alpm,
    positionals: &[String],
    as_deps: bool,
    json: bool,
    approvals: Option<crate::question::Approvals>,
) -> anyhow::Result<()> {
    let targets = positionals
        .iter()
        .map(|s| classify_target(s))
        .collect::<Vec<_>>();
    let needs_lookup = targets.iter().any(|t| matches!(t, InstallTarget::Repo(_)));
    if needs_lookup {
        for target in &targets {
            if let InstallTarget::Repo(name) = target
                && crate::pacman::find_pkg(handle, name).is_none()
            {
                anyhow::bail!("cannot build packages as root; re-run without privilege escalation");
            }
        }
    }
    let interactive = approvals.is_none() && stdin_is_tty();
    let answerer = answerer_for(approvals);
    if json {
        if interactive {
            install::run_install(
                &targets,
                as_deps,
                EscalatedSink::new(),
                confirm_install_stderr,
                answerer,
            )
        } else {
            install::run_install(&targets, as_deps, JsonSink::new(), || true, answerer)
        }
    } else {
        install::run_install(
            &targets,
            as_deps,
            ConsoleSink::new(),
            confirm_install,
            answerer,
        )
    }
}
