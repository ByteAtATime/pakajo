use super::privs::stdin_is_tty;
use crate::package::PackageSource;
use crate::search::SearchResult;

pub fn run_aur_sync() -> anyhow::Result<()> {
    let handle = crate::pacman::handle()?;
    let index = crate::db::PackageDb::open(&crate::db::PackageDb::db_path()?)?;
    match index.refresh(&handle)? {
        crate::db::RefreshOutcome::NotModified => println!("index up to date"),
        crate::db::RefreshOutcome::Updated {
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
    let handle = crate::pacman::handle()?;
    let aur = crate::aur::AurClient::new();
    match crate::devel::generate_db(&handle, &aur)? {
        crate::devel::GendbOutcome::NoForeign => {
            println!(
                "no foreign packages installed; wrote {}",
                crate::devel::state_path().display()
            );
        }
        crate::devel::GendbOutcome::LookupFailed => {}
        crate::devel::GendbOutcome::Recorded(recorded) => {
            let path = crate::devel::state_path();
            println!("recorded {recorded} devel package(s) to {}", path.display());
        }
    }
    Ok(())
}

pub fn run_search(query: &str) -> anyhow::Result<()> {
    let sqlite_path = crate::db::PackageDb::db_path()?;
    let local = crate::db::PackageDb::open(&sqlite_path)?;
    refresh_index_if_stale(&local);
    let engine = crate::search::engine::SearchEngine::new(sqlite_path)?;
    let snapshot = crate::pacman::snapshot::get()?;
    let installed: std::collections::HashSet<String> = snapshot.installed.into_iter().collect();
    let results = crate::search::dispatch_search(
        &engine,
        &local,
        &installed,
        query,
        &snapshot.groups,
        crate::search::SearchFilter::All,
    );
    print_search_results(&results);
    Ok(())
}

fn refresh_index_if_stale(local: &crate::db::PackageDb) {
    if let Some(age) = local.last_refreshed_age()
        && age < crate::db::AUR_SYNC_MIN_INTERVAL
    {
        return;
    }
    let handle = match crate::pacman::handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("warning: skipping index refresh: {e:#}");
            return;
        }
    };
    let spinner = super::spinner::Spinner::start("refreshing package index");
    let outcome = local.refresh(&handle);
    spinner.stop();
    match outcome {
        Ok(crate::db::RefreshOutcome::NotModified) => {}
        Ok(crate::db::RefreshOutcome::Updated {
            aur_count,
            repo_count,
            skipped,
        }) => {
            eprintln!("indexed {aur_count} aur + {repo_count} repo packages (skipped {skipped})");
        }
        Err(e) => {
            eprintln!("warning: index refresh failed, searching stale index: {e:#}");
        }
    }
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
