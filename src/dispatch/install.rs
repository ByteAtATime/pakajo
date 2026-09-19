use alpm_utils::DbListExt;
use anyhow::Context as _;

use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::{ChildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;
use crate::dispatch::remove::Preview;
use crate::dispatch::session::{PhasePlan, run_phases};
use crate::resolve::{ConflictReport, Decisions, Plan, RepoInstall};

pub struct InstallRequest {
    pub targets: Vec<String>,
    pub as_deps: bool,
    pub no_check: bool,
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
    let config = crate::pacman::config()?;
    let mut handle = crate::pacman::handle_with_config(&config)?;
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
    let (files, names) = peel_file_targets(&expanded);
    let plan = resolve_combined_plan(&names, request.no_check)?;
    handle
        .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
        .context("failed to init install preview transaction")?;
    queue_file_targets(handle, &files)?;
    queue_plan_repo_installs(handle, plan.as_ref())?;
    queue_stub_targets(handle, plan.as_ref())?;
    let prepare_error = handle
        .trans_prepare()
        .err()
        .map(crate::dry_run::extract_prepare_failure);
    let mut questions = crate::dry_run::snapshot(state);
    let engine_sourced = plan
        .as_ref()
        .map(|plan| plan_conflicts_to_questions(&plan.conflicts))
        .unwrap_or_default();
    questions.conflicts = merge_conflicts(&questions.conflicts, &engine_sourced);
    let summary = crate::install::build_summary(handle);
    Ok(Preview {
        summary,
        questions,
        prepare_error,
        aur: Vec::new(),
        pkgbuild_diffs: Vec::new(),
    })
}

fn merge_conflicts(
    existing: &[crate::question::Conflict],
    engine_sourced: &[crate::question::Conflict],
) -> Vec<crate::question::Conflict> {
    let mut merged = existing.to_vec();
    for conflict in engine_sourced {
        if !merged.contains(conflict) {
            merged.push(conflict.clone());
        }
    }
    merged
}

fn resolve_combined_plan(names: &[String], no_check: bool) -> anyhow::Result<Option<Plan>> {
    if names.is_empty() {
        return Ok(None);
    }
    let plan = crate::resolve::resolve_plan(names, no_check, Decisions::Default)?;
    Ok(Some(plan))
}

fn queue_file_targets(handle: &mut alpm::Alpm, files: &[String]) -> anyhow::Result<()> {
    for target in files {
        let loaded = handle
            .pkg_load(
                target.as_str(),
                true,
                crate::pacman::local_file_siglevel(handle),
            )
            .context("failed to load package file")?;
        handle
            .trans_add_pkg(loaded)
            .map_err(alpm::Error::from)
            .context("failed to queue package file for installation")?;
    }
    Ok(())
}

fn queue_plan_repo_installs(handle: &mut alpm::Alpm, plan: Option<&Plan>) -> anyhow::Result<()> {
    let Some(plan) = plan else {
        return Ok(());
    };
    for row in &plan.repo_installs {
        let pkg = plan_repo_pkg(handle, row)?;
        handle
            .trans_add_pkg(pkg)
            .map_err(alpm::Error::from)
            .context("failed to queue package for installation")?;
    }
    Ok(())
}

fn plan_repo_pkg<'a>(
    handle: &'a alpm::Alpm,
    row: &RepoInstall,
) -> anyhow::Result<&'a alpm::Package> {
    if row.db.is_empty() {
        return handle
            .syncdbs()
            .pkg(row.name.as_str())
            .map_err(|_| anyhow::anyhow!("package '{}' not found in any repository", row.name));
    }
    let pin = format!("{}/{}", row.db, row.name);
    handle
        .syncdbs()
        .find_target(pin.as_str())
        .map_err(|_| anyhow::anyhow!("package '{}' not found in any repository", row.name))
}

pub(crate) fn queue_stub_targets(
    handle: &mut alpm::Alpm,
    plan: Option<&Plan>,
) -> anyhow::Result<()> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let stub_dir = tempfile::tempdir().context("failed to create stub work dir")?;
    for (_, members) in plan.aur_builds() {
        for member in members {
            let path =
                crate::stub_pkg::build_stub_pkg(&member.name, &member.version, stub_dir.path())
                    .with_context(|| format!("failed to build stub for {}", member.name))?;
            let loaded = handle
                .pkg_load(path.to_string_lossy().as_ref(), false, alpm::SigLevel::NONE)
                .with_context(|| format!("failed to load stub for {}", member.name))?;
            handle
                .trans_add_pkg(loaded)
                .map_err(alpm::Error::from)
                .with_context(|| format!("failed to queue stub for {}", member.name))?;
        }
    }
    Ok(())
}

fn unwrap_or_fail<T>(
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
    result: anyhow::Result<T>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            None
        }
    }
}

fn seal_approvals(
    request: &InstallRequest,
) -> anyhow::Result<Option<crate::dispatch::approvals::ApprovalsFile>> {
    request
        .approvals
        .as_deref()
        .map(|payload| crate::dispatch::approvals::ApprovalsFile::write(payload.as_bytes()))
        .transpose()
}

fn run_install(request: InstallRequest, mut tx: futures::channel::mpsc::Sender<StreamItem>) {
    if crate::cli::privs::is_root() {
        run_root_install(request, &mut tx);
        return;
    }
    let mut handle = match crate::pacman::handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let peeled = {
        let peeled = unwrap_or_fail(&mut tx, peel_for_dispatch(&mut handle, &request));
        drop(handle);
        peeled
    };
    let Some(peeled) = peeled else {
        return;
    };
    let files_only = peeled.names.is_empty();
    let privileged = if files_only {
        if peeled.files.is_empty() {
            send_done(
                &mut tx,
                ChildOutcome::Failed("no install targets".to_string()),
            );
            return;
        }
        let Some(sealed) = unwrap_or_fail(&mut tx, seal_approvals(&request)) else {
            return;
        };
        Some(PrivilegedOperation::Install {
            targets: peeled.files.clone(),
            as_deps: request.as_deps,
            preconfirmed: false,
            approvals: sealed,
        })
    } else {
        None
    };
    run_phases(
        PhasePlan {
            privileged,
            aur_targets: if files_only { Vec::new() } else { peeled.names },
            files: if files_only { Vec::new() } else { peeled.files },
            as_deps: request.as_deps,
            no_check: request.no_check,
            repo_verb: "installed",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
        },
        &mut tx,
    );
}

fn run_root_install(request: InstallRequest, tx: &mut futures::channel::mpsc::Sender<StreamItem>) {
    let mut handle = match crate::pacman::handle() {
        Ok(handle) => handle,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let peeled = {
        let peeled = unwrap_or_fail(tx, peel_for_dispatch(&mut handle, &request));
        drop(handle);
        peeled
    };
    let Some(peeled) = peeled else {
        return;
    };
    if peeled.names.is_empty() {
        let Some(sealed) = unwrap_or_fail(tx, seal_approvals(&request)) else {
            return;
        };
        let operation = ChildOperation::Install {
            targets: peeled.files,
            as_deps: request.as_deps,
            preconfirmed: false,
            approvals_path: sealed
                .as_ref()
                .map(|file| file.path().to_string_lossy().into_owned()),
            stream: request.json,
        };
        let outcome = operation
            .execute()
            .map(|()| ChildOutcome::Success)
            .unwrap_or_else(|error| ChildOutcome::Failed(format!("{error:#}")));
        send_done(tx, outcome);
        return;
    }
    run_phases(
        PhasePlan {
            privileged: None,
            aur_targets: peeled.names,
            files: peeled.files,
            as_deps: request.as_deps,
            no_check: request.no_check,
            repo_verb: "installed",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
        },
        tx,
    );
}

struct PeeledTargets {
    files: Vec<String>,
    names: Vec<String>,
}

fn peel_for_dispatch(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
) -> anyhow::Result<PeeledTargets> {
    let config = crate::pacman::config()?;
    crate::upgrade::apply_ignores(handle, &config, &request.ignores);
    let expanded = expand_install_groups(handle, &request.targets, request.tty);
    let (files, names) = peel_file_targets(&expanded);
    Ok(PeeledTargets { files, names })
}

fn plan_conflicts_to_questions(report: &ConflictReport) -> Vec<crate::question::Conflict> {
    report
        .local
        .iter()
        .chain(report.inner.iter())
        .flat_map(|conflict| {
            conflict
                .conflicting
                .iter()
                .map(|entry| crate::question::Conflict {
                    incoming: conflict.pkg.clone(),
                    removable: entry.pkg.clone(),
                })
        })
        .collect()
}

fn expand_install_groups(
    handle: &alpm::Alpm,
    positionals: &[String],
    interactive: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in positionals {
        if crate::package::repo_exists(handle, s) {
            out.push(s.clone());
            continue;
        }
        let groups = crate::package::find_groups(handle, s);
        if groups.is_empty() {
            out.push(s.clone());
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
        let mut expansion_seen: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for name in members {
            if expansion_seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

fn peel_file_targets(positionals: &[String]) -> (Vec<String>, Vec<String>) {
    let mut files: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for s in positionals {
        if file_suffix_path(s).is_some() {
            files.push(s.clone());
        } else {
            names.push(s.clone());
        }
    }
    (files, names)
}

fn file_suffix_path(target: &str) -> Option<&str> {
    const FILE_SUFFIXES: &[&str] = &[".pkg.tar", ".pkg.tar.gz", ".pkg.tar.zst", ".pkg.tar.xz"];
    FILE_SUFFIXES
        .iter()
        .any(|suffix| target.ends_with(suffix))
        .then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Conflict, Conflicting, RepoInstall};

    fn conflicting_report() -> ConflictReport {
        ConflictReport {
            local: vec![Conflict {
                pkg: "nvidia-470xx-utils".to_string(),
                conflicting: vec![Conflicting {
                    pkg: "nvidia-utils".to_string(),
                    conflict: Some("nvidia-utils".to_string()),
                }],
            }],
            inner: vec![Conflict {
                pkg: "cava-git".to_string(),
                conflicting: vec![
                    Conflicting {
                        pkg: "cava".to_string(),
                        conflict: None,
                    },
                    Conflicting {
                        pkg: "cava-old".to_string(),
                        conflict: Some("cava=1.0".to_string()),
                    },
                ],
            }],
        }
    }

    #[test]
    fn peel_file_targets_separates_suffix_paths_from_names() {
        let (files, names) = peel_file_targets(&[
            "neovim".to_string(),
            "/tmp/foo-1.0-1-x86_64.pkg.tar.zst".to_string(),
            "yay-bin".to_string(),
        ]);
        assert_eq!(files, vec!["/tmp/foo-1.0-1-x86_64.pkg.tar.zst".to_string()]);
        assert_eq!(names, vec!["neovim".to_string(), "yay-bin".to_string()]);
    }

    #[test]
    fn merge_conflicts_concatenates_disjoint_sets() {
        let existing = vec![crate::question::Conflict {
            incoming: "linux".to_string(),
            removable: "linux-lts".to_string(),
        }];
        let engine = vec![
            crate::question::Conflict {
                incoming: "nvidia-470xx-utils".to_string(),
                removable: "nvidia-utils".to_string(),
            },
            crate::question::Conflict {
                incoming: "cava-git".to_string(),
                removable: "cava".to_string(),
            },
        ];
        assert_eq!(
            merge_conflicts(&existing, &engine),
            vec![
                crate::question::Conflict {
                    incoming: "linux".to_string(),
                    removable: "linux-lts".to_string(),
                },
                crate::question::Conflict {
                    incoming: "nvidia-470xx-utils".to_string(),
                    removable: "nvidia-utils".to_string(),
                },
                crate::question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                },
            ]
        );
    }

    #[test]
    fn merge_conflicts_drops_exact_pair_duplicates() {
        let existing = vec![crate::question::Conflict {
            incoming: "nvidia-470xx-utils".to_string(),
            removable: "nvidia-utils".to_string(),
        }];
        let engine = vec![
            crate::question::Conflict {
                incoming: "nvidia-470xx-utils".to_string(),
                removable: "nvidia-utils".to_string(),
            },
            crate::question::Conflict {
                incoming: "cava-git".to_string(),
                removable: "cava".to_string(),
            },
        ];
        assert_eq!(
            merge_conflicts(&existing, &engine),
            vec![
                crate::question::Conflict {
                    incoming: "nvidia-470xx-utils".to_string(),
                    removable: "nvidia-utils".to_string(),
                },
                crate::question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                },
            ]
        );
    }

    #[test]
    fn merge_conflicts_keeps_non_conflicting_recorder_entries() {
        let existing = vec![
            crate::question::Conflict {
                incoming: "linux".to_string(),
                removable: "linux-lts".to_string(),
            },
            crate::question::Conflict {
                incoming: "nvidia-470xx-utils".to_string(),
                removable: "nvidia-utils".to_string(),
            },
        ];
        let engine = vec![crate::question::Conflict {
            incoming: "nvidia-470xx-utils".to_string(),
            removable: "nvidia-utils".to_string(),
        }];
        assert_eq!(
            merge_conflicts(&existing, &engine),
            vec![
                crate::question::Conflict {
                    incoming: "linux".to_string(),
                    removable: "linux-lts".to_string(),
                },
                crate::question::Conflict {
                    incoming: "nvidia-470xx-utils".to_string(),
                    removable: "nvidia-utils".to_string(),
                },
            ]
        );
    }

    #[test]
    fn conflict_mapping_flattens_each_entry() {
        let mapped = plan_conflicts_to_questions(&conflicting_report());
        assert_eq!(
            mapped,
            vec![
                crate::question::Conflict {
                    incoming: "nvidia-470xx-utils".to_string(),
                    removable: "nvidia-utils".to_string(),
                },
                crate::question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                },
                crate::question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava-old".to_string(),
                },
            ]
        );
    }

    #[test]
    fn conflict_mapping_empty_when_no_conflicts() {
        let mapped = plan_conflicts_to_questions(&ConflictReport::default());
        assert!(mapped.is_empty());
    }
}
