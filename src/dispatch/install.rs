use anyhow::Context as _;
use futures::SinkExt as _;

use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem};
use crate::dispatch::operation::ChildOperation;
use crate::dispatch::protocol::Decider;
use crate::dispatch::remove::Preview;
use crate::dispatch::session::{PhasePlan, run_phases};

pub struct InstallRequest {
    pub targets: Vec<String>,
    pub as_deps: bool,
    pub ignores: Vec<String>,
    pub prefer_aur: bool,
    pub decider: Box<dyn Decider + Send>,
    pub approvals: Option<String>,
    pub tty: bool,
    pub json: bool,
}

pub fn install(request: InstallRequest) -> DispatchStream {
    let (tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || run_install(request, tx));
    rx
}

pub fn install_preview(request: &InstallRequest) -> anyhow::Result<Preview> {
    let mut handle = crate::cli::alpm_handle()?;
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores);
    let state = crate::dry_run::attach_recorder(&mut handle);
    let outcome = run_install_preview(&mut handle, request, &state);
    let _ = handle.trans_release();
    outcome
}

pub(crate) fn run_install_preview(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
    state: &std::rc::Rc<std::cell::RefCell<crate::dry_run::RecorderState>>,
) -> anyhow::Result<Preview> {
    let expanded = expand_install_groups(handle, &request.targets, request.tty);
    let (repo_or_file, aur) = split_install_targets(handle, &expanded, request.prefer_aur);
    let plan = resolve_aur_plan(handle, &aur)?;
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init install preview transaction")?;
    queue_repo_targets(handle, &repo_or_file)?;
    queue_stub_targets(handle, &plan)?;
    let prepare_error = handle
        .trans_prepare()
        .err()
        .map(crate::dry_run::extract_prepare_failure);
    let questions = crate::dry_run::snapshot(state);
    let summary = crate::install::build_summary(handle);
    Ok(Preview {
        summary,
        questions,
        prepare_error,
        aur: Vec::new(),
        pkgbuild_diffs: Vec::new(),
    })
}

fn resolve_aur_plan(
    handle: &alpm::Alpm,
    aur: &[String],
) -> anyhow::Result<Option<crate::resolve::BuildPlan>> {
    if aur.is_empty() {
        return Ok(None);
    }
    let client = crate::aur::AurClient::new();
    crate::resolve::resolve(&crate::resolve::AlpmDb(handle), &client, aur, false).map(Some)
}

fn queue_repo_targets(handle: &mut alpm::Alpm, repo_or_file: &[String]) -> anyhow::Result<()> {
    for target in repo_or_file {
        if let Some(path) = file_suffix_path(target) {
            let loaded = handle
                .pkg_load(path, true, alpm::SigLevel::NONE)
                .context("failed to load package file")?;
            handle
                .trans_add_pkg(loaded)
                .map_err(alpm::Error::from)
                .context("failed to queue package file for installation")?;
            continue;
        }
        let pkg = crate::pacman::find_pkg(handle, target)
            .ok_or_else(|| anyhow::anyhow!("package '{target}' not found in any repository"))?;
        handle
            .trans_add_pkg(pkg)
            .map_err(alpm::Error::from)
            .context("failed to queue package for installation")?;
    }
    Ok(())
}

pub(crate) fn queue_stub_targets(
    handle: &mut alpm::Alpm,
    plan: &Option<crate::resolve::BuildPlan>,
) -> anyhow::Result<()> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let stub_dir = tempfile::tempdir().context("failed to create stub work dir")?;
    for layer in &plan.layers {
        for info in &layer.aur {
            let path = crate::stub_pkg::build_stub_pkg(info, stub_dir.path())
                .with_context(|| format!("failed to build stub for {}", info.name))?;
            let loaded = handle
                .pkg_load(path.to_string_lossy().as_ref(), false, alpm::SigLevel::NONE)
                .with_context(|| format!("failed to load stub for {}", info.name))?;
            handle
                .trans_add_pkg(loaded)
                .map_err(alpm::Error::from)
                .with_context(|| format!("failed to queue stub for {}", info.name))?;
        }
    }
    Ok(())
}

fn run_install(request: InstallRequest, mut tx: futures::channel::mpsc::Sender<StreamItem>) {
    if crate::cli::privs::is_root() {
        run_root_install(request, &mut tx);
        return;
    }
    let mut handle = match crate::cli::alpm_handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let resolved = match resolve_for_dispatch(&mut handle, &request) {
        Ok(resolved) => resolved,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    drop(handle);
    let sealed = match request
        .approvals
        .as_deref()
        .map(|payload| crate::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
        .transpose()
    {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    run_phases(
        PhasePlan {
            repo_targets: resolved.repo_or_file,
            aur_targets: resolved.aur,
            as_deps: request.as_deps,
            approvals: sealed,
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
        },
        &mut tx,
    );
}

fn run_root_install(request: InstallRequest, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    let mut handle = match crate::cli::alpm_handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let resolved = match resolve_for_dispatch(&mut handle, &request) {
        Ok(resolved) => resolved,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    drop(handle);
    if !resolved.aur.is_empty() {
        send_done(
            tx,
            ChildOutcome::Failed(
                "cannot build packages as root; re-run without privilege escalation".to_string(),
            ),
        );
        return;
    }
    let sealed = match request
        .approvals
        .as_deref()
        .map(|payload| crate::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
        .transpose()
    {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let operation = ChildOperation::Install {
        targets: resolved.repo_or_file,
        as_deps: request.as_deps,
        approvals_path: sealed
            .as_ref()
            .map(|file| file.path().to_string_lossy().into_owned()),
        stream: request.json,
    };
    let outcome = match operation.execute() {
        Ok(()) => ChildOutcome::Success,
        Err(error) => ChildOutcome::Failed(format!("{error:#}")),
    };
    send_done(tx, outcome);
}

struct ResolvedTargets {
    repo_or_file: Vec<String>,
    aur: Vec<String>,
}

fn resolve_for_dispatch(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
) -> anyhow::Result<ResolvedTargets> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    crate::upgrade::apply_ignores(handle, &config, &request.ignores);
    let expanded = expand_install_groups(handle, &request.targets, request.tty);
    let (repo_or_file, aur) = split_install_targets(handle, &expanded, request.prefer_aur);
    if request.tty && !request.json && !repo_or_file.is_empty() {
        print_sync_preamble(handle, &repo_or_file);
    }
    Ok(ResolvedTargets { repo_or_file, aur })
}

fn expand_install_groups(
    handle: &alpm::Alpm,
    positionals: &[String],
    interactive: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in positionals {
        if crate::package::repo_exists(handle, s) {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        let groups = crate::package::find_groups(handle, s);
        if groups.is_empty() {
            if seen.insert(s.clone()) {
                out.push(s.clone());
            }
            continue;
        }
        let members: Vec<String> = if interactive {
            crate::cli::prompts::select_group_members(s, &groups)
        } else {
            groups
                .iter()
                .flat_map(|g| g.members.iter())
                .map(|m| m.name.clone())
                .collect()
        };
        for name in members {
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

fn split_install_targets(
    handle: &alpm::Alpm,
    positionals: &[String],
    prefer_aur: bool,
) -> (Vec<String>, Vec<String>) {
    let mut repo_or_file: Vec<String> = Vec::new();
    let mut aur: Vec<String> = Vec::new();
    for s in positionals {
        if file_suffix_path(s).is_some() {
            repo_or_file.push(s.clone());
            continue;
        }
        if prefer_aur {
            aur.push(s.clone());
            continue;
        }
        if crate::package::repo_exists(handle, s) {
            repo_or_file.push(s.clone());
        } else {
            aur.push(s.clone());
        }
    }
    (repo_or_file, aur)
}

fn file_suffix_path(target: &str) -> Option<&str> {
    const FILE_SUFFIXES: &[&str] = &[".pkg.tar", ".pkg.tar.gz", ".pkg.tar.zst", ".pkg.tar.xz"];
    FILE_SUFFIXES
        .iter()
        .any(|suffix| target.ends_with(suffix))
        .then_some(target)
}

fn print_sync_preamble(handle: &alpm::Alpm, targets: &[String]) {
    let labeled: Vec<String> = targets
        .iter()
        .map(|name| match crate::package::find(handle, name) {
            Some(pkg) => format!("{name}-{}", pkg.version),
            None => name.clone(),
        })
        .collect();
    let c = crate::color::stdout_color();
    println!(
        "{} {}",
        crate::color::paint(
            c,
            crate::color::BOLD,
            &format!("Sync Explicit ({}):", targets.len())
        ),
        crate::color::paint(c, crate::color::CYAN, &labeled.join(", "))
    );
}

fn send_done(tx: &mut futures::channel::mpsc::Sender<StreamItem>, outcome: ChildOutcome) {
    if let Err(error) = futures::executor::block_on(tx.send(StreamItem::Done(outcome))) {
        eprintln!("warning: dispatch stream closed: {error}");
    }
}
