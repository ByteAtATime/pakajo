use std::sync::Arc;

use anyhow::Context as _;
use base64::Engine as _;

use super::classify_target;
use super::privs::stdin_is_tty;
use super::prompts::{confirm_install, confirm_install_stderr};
use super::sinks::{ConsoleSink, EscalatedSink, JsonSink};
use crate::aur::AurClient;
use crate::install::{self, InstallTarget};
use crate::search::{AurSearchProvider, RepoSearchIndex, RepoSearchProvider, SearchResult};

pub(super) fn alpm_handle() -> anyhow::Result<alpm::Alpm> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    crate::pacman::init_alpm(&config)
}

pub(super) fn run_aur_sync() -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let index = crate::local_index::LocalIndex::open(&crate::local_index::LocalIndex::db_path()?)?;
    match index.refresh(&handle)? {
        crate::local_index::RefreshOutcome::NotModified => println!("index up to date"),
        crate::local_index::RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        } => {
            println!("indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})");
        }
    }
    Ok(())
}

pub(super) fn run_gendb() -> anyhow::Result<()> {
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

pub(super) fn run_search(query: &str) -> anyhow::Result<()> {
    let handle = alpm_handle()?;
    let installed = crate::package::installed_names(&handle);
    let local_index = crate::local_index::LocalIndex::db_path()
        .ok()
        .and_then(|p| crate::local_index::LocalIndex::open(&p).ok())
        .map(Arc::new);
    let index = RepoSearchIndex::from_alpm(&handle);
    let repo_provider = RepoSearchProvider::new(Arc::new(index));
    let aur_provider = AurSearchProvider::new(Arc::new(AurClient::new()));
    let outcome = crate::search::dispatch_search(
        local_index,
        &repo_provider,
        &aur_provider,
        &installed,
        query,
    );
    print_search_results(&outcome.results);
    if let Some(err) = &outcome.aur_error {
        eprintln!("  aur: {err}");
    }
    Ok(())
}

fn print_search_results(rows: &[SearchResult]) {
    if rows.is_empty() {
        return;
    }
    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let version_width = rows.iter().map(|r| r.version.len()).max().unwrap_or(0);
    for row in rows {
        let repo = row.repo.as_deref().unwrap_or("-");
        let desc = row.description.as_deref().unwrap_or("-");
        println!(
            "  {:<nw$}  {:<vw$}  [{}]  {}",
            row.name,
            row.version,
            repo,
            desc,
            nw = name_width,
            vw = version_width,
        );
    }
}

pub(super) fn decode_approvals(b64: &str) -> anyhow::Result<crate::question::Approvals> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("--approvals is not valid base64")?;
    serde_json::from_slice(&bytes).context("--approvals is not valid JSON")
}

pub(super) fn answerer_for(
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

pub(super) fn root_install(
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
        let handle = alpm_handle()?;
        for target in &targets {
            if let InstallTarget::Repo(name) = target
                && crate::pacman::find_pkg(&handle, name).is_none()
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
