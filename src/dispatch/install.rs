use anyhow::Context as _;

use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::{ChildOperation, PrivilegedOperation};
use crate::dispatch::protocol::Decider;
use crate::dispatch::seal::{JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing};
use crate::dispatch::session::{PhasePlan, run_phases};
use crate::question::model::Question;
use crate::question::review::Review;
use crate::question::source::{AnswerSource, ExploreDefaults};
use crate::resolve::{ConflictReport, Decisions, Plan};
use crate::tx::driver::{Finish, RunKind, RunSpec};
use crate::tx::targets::peel_file_targets;

pub struct InstallRequest {
    pub targets: Vec<String>,
    pub as_deps: bool,
    pub reinstall: bool,
    pub no_check: bool,
    pub ignores: Vec<String>,
    pub prefer_aur: bool,
    pub decider: Box<dyn Decider + Send>,
    pub approvals: Option<String>,
    pub tty: bool,
    pub json: bool,
}

pub struct InstallPreview {
    pub review: Review,
    pub aur: Vec<crate::upgrade::AurUpgradeCandidate>,
    pub pkgbuild_diffs: Vec<crate::pkgbuild::PkgbuildDiff>,
    pub prepare_error: Option<crate::tx::convert::PrepareFailure>,
}

pub(crate) const NON_INTERACTIVE_SEAL_REQUIRED: &str =
    "non-interactive install requires sealed approvals";

pub fn install(request: InstallRequest) -> DispatchStream {
    let (mut tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || {
        if json_seal_missing(request.json, request.approvals.as_deref()) {
            send_done(
                &mut tx,
                ChildOutcome::Failed(JSON_SEAL_REQUIRED.to_string()),
            );
            return;
        }
        if non_interactive_seal_missing(request.json, request.tty, request.approvals.as_deref()) {
            send_done(
                &mut tx,
                ChildOutcome::Failed(NON_INTERACTIVE_SEAL_REQUIRED.to_string()),
            );
            return;
        }
        run_install(request, tx);
    });
    rx
}

pub(crate) fn run_install_preview(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
) -> anyhow::Result<InstallPreview> {
    run_install_preview_with(handle, request, Box::new(ExploreDefaults))
}

pub(crate) fn run_install_preview_with(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
    source: Box<dyn AnswerSource>,
) -> anyhow::Result<InstallPreview> {
    let expanded = expand_install_groups(handle, &request.targets, request.tty);
    let (files, names) = peel_file_targets(&expanded);
    let plan = resolve_combined_plan(&names, request.no_check)?;
    let (_stub_dir, stubs) = build_stubs(plan.as_ref())?;
    let mut targets = files;
    targets.extend(repo_target_names(plan.as_ref())?);
    let spec = RunSpec {
        kind: RunKind::Sync,
        targets,
        stub_targets: stubs,
        explore: true,
        as_deps: request.as_deps,
        reinstall: request.reinstall,
        dep_names: Vec::new(),
    };
    let outcome = crate::tx::compose::preview_with(handle, spec, source)?;
    let review = outcome
        .review
        .context("install preview produced no review")?;
    let review = merge_plan_conflicts(review, plan.as_ref());
    let prepare_error = match outcome.finish {
        Finish::PrepareFailed(failure) => Some(failure),
        _ => None,
    };
    Ok(InstallPreview {
        review,
        aur: Vec::new(),
        pkgbuild_diffs: Vec::new(),
        prepare_error,
    })
}

fn repo_target_names(plan: Option<&Plan>) -> anyhow::Result<Vec<String>> {
    let Some(plan) = plan else {
        return Ok(Vec::new());
    };
    plan.repo_installs
        .iter()
        .map(|row| {
            if row.db.is_empty() {
                anyhow::bail!("resolver produced no database for package {}", row.name);
            }
            Ok(format!("{}/{}={}", row.db, row.name, row.version))
        })
        .collect()
}

fn merge_plan_conflicts(mut review: Review, plan: Option<&Plan>) -> Review {
    let Some(plan) = plan else {
        return review;
    };
    for conflict in plan_conflicts_to_questions(&plan.conflicts) {
        let question = Question::Conflict {
            incoming: conflict.incoming,
            removable: conflict.removable,
        };
        if !review.part1.contains(&question) {
            review.part1.push(question);
        }
    }
    review
}

pub(crate) fn build_stubs(
    plan: Option<&Plan>,
) -> anyhow::Result<(Option<tempfile::TempDir>, Vec<String>)> {
    let Some(plan) = plan else {
        return Ok((None, Vec::new()));
    };
    let stub_dir = tempfile::tempdir().context("failed to create stub work dir")?;
    let mut paths = Vec::new();
    for (_, members) in plan.aur_builds() {
        for member in members {
            let path =
                crate::stub_pkg::build_stub_pkg(&member.name, &member.version, stub_dir.path())
                    .with_context(|| format!("failed to build stub for {}", member.name))?;
            paths.push(path.to_string_lossy().into_owned());
        }
    }
    Ok((Some(stub_dir), paths))
}

fn resolve_combined_plan(names: &[String], no_check: bool) -> anyhow::Result<Option<Plan>> {
    if names.is_empty() {
        return Ok(None);
    }
    let plan = crate::resolve::resolve_plan(names, no_check, Decisions::Default)?;
    Ok(Some(plan))
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
    let Some(targets) = direct_install_targets(&peeled) else {
        run_phases(
            PhasePlan {
                privileged: None,
                repo_committed: false,
                aur_targets: peeled.names,
                files: peeled.files,
                as_deps: request.as_deps,
                reinstall: peeled.reinstall,
                no_check: request.no_check,
                repo_verb: "installed",
                approvals_payload: request.approvals,
                decider: request.decider,
                tty: request.tty,
                interactive: request.tty && !request.json,
            },
            &mut tx,
        );
        return;
    };
    if targets.is_empty() {
        send_done(
            &mut tx,
            ChildOutcome::Failed("no install targets".to_string()),
        );
        return;
    }
    let Some(sealed) = unwrap_or_fail(&mut tx, seal_approvals(&request)) else {
        return;
    };
    run_phases(
        PhasePlan {
            privileged: Some(PrivilegedOperation::Install {
                targets,
                as_deps: request.as_deps,
                reinstall: peeled.reinstall,
                preconfirmed: false,
                interactive: request.tty && !request.json,
                approvals: sealed,
            }),
            repo_committed: false,
            aur_targets: Vec::new(),
            files: Vec::new(),
            as_deps: request.as_deps,
            reinstall: peeled.reinstall,
            no_check: request.no_check,
            repo_verb: "installed",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
            interactive: request.tty && !request.json,
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
    if let Some(targets) = direct_install_targets(&peeled) {
        if targets.is_empty() {
            send_done(tx, ChildOutcome::Failed("no install targets".to_string()));
            return;
        }
        let Some(sealed) = unwrap_or_fail(tx, seal_approvals(&request)) else {
            return;
        };
        let operation = ChildOperation::Install {
            targets,
            as_deps: request.as_deps,
            reinstall: peeled.reinstall,
            preconfirmed: false,
            interactive: request.tty && !request.json,
            approvals_path: sealed
                .as_ref()
                .map(|file| file.path().to_string_lossy().into_owned()),
            stream: request.json,
        };
        let outcome = operation
            .execute()
            .map(|code| {
                crate::dispatch::exec::map_exit_code(
                    crate::dispatch::exec::ChildKind::Install,
                    code,
                    "direct",
                )
            })
            .unwrap_or_else(|error| ChildOutcome::Failed(format!("{error:#}")));
        send_done(tx, outcome);
        return;
    }
    run_phases(
        PhasePlan {
            privileged: None,
            repo_committed: false,
            aur_targets: peeled.names,
            files: peeled.files,
            as_deps: request.as_deps,
            reinstall: peeled.reinstall,
            no_check: request.no_check,
            repo_verb: "installed",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
            interactive: request.tty && !request.json,
        },
        tx,
    );
}

struct PeeledTargets {
    files: Vec<String>,
    names: Vec<String>,
    pure_repo: bool,
    reinstall: bool,
}

fn direct_install_targets(peeled: &PeeledTargets) -> Option<Vec<String>> {
    if !peeled.names.is_empty() && !peeled.pure_repo {
        return None;
    }
    let mut targets = peeled.files.clone();
    targets.extend(peeled.names.iter().cloned());
    Some(targets)
}

fn peel_for_dispatch(
    handle: &mut alpm::Alpm,
    request: &InstallRequest,
) -> anyhow::Result<PeeledTargets> {
    let config = crate::pacman::config()?;
    crate::upgrade::apply_ignores(handle, &config, &request.ignores);
    let expanded = expand_install_groups(handle, &request.targets, request.tty);
    let (files, names) = peel_file_targets(&expanded);
    let pure_repo = names.iter().all(|name| routes_to_engine(handle, name));
    Ok(PeeledTargets {
        files,
        names,
        pure_repo,
        reinstall: request.reinstall,
    })
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

fn repo_resolvable(handle: &alpm::Alpm, target: &str) -> bool {
    crate::tx::targets::unresolvable_target(handle, std::slice::from_ref(&target.to_string()))
        .is_none()
}

fn routes_to_engine(handle: &alpm::Alpm, target: &str) -> bool {
    repo_resolvable(handle, target) || !crate::package::find_groups(handle, target).is_empty()
}

fn group_members(handle: &alpm::Alpm, target: &str) -> Vec<String> {
    let mut members: Vec<String> = Vec::new();
    for group in crate::package::find_groups(handle, target) {
        for member in group.members {
            if !members.contains(&member.name) {
                members.push(member.name);
            }
        }
    }
    members
}

fn expand_install_groups(handle: &alpm::Alpm, positionals: &[String], tty: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for target in positionals {
        if !tty && !repo_resolvable(handle, target) {
            let members = group_members(handle, target);
            if !members.is_empty() {
                out.extend(members);
                continue;
            }
        }
        out.push(target.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::seal::{
        JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing,
    };
    use crate::resolve::{Conflict, Conflicting};

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
    fn missing_seals_are_rejected_with_exact_messages() {
        assert!(json_seal_missing(true, None));
        assert!(!json_seal_missing(true, Some("{}")));
        assert!(!json_seal_missing(false, None));
        assert_eq!(JSON_SEAL_REQUIRED, "--json requires sealed approvals");
        assert!(non_interactive_seal_missing(false, false, None));
        assert!(!non_interactive_seal_missing(false, true, None));
        assert!(!non_interactive_seal_missing(true, false, None));
        assert!(!non_interactive_seal_missing(false, false, Some("{}")));
        assert_eq!(
            NON_INTERACTIVE_SEAL_REQUIRED,
            "non-interactive install requires sealed approvals"
        );
    }

    #[test]
    fn merge_plan_conflicts_dedupes_while_preserving_order() {
        let linux = Question::Conflict {
            incoming: "linux".to_string(),
            removable: "linux-lts".to_string(),
        };
        let nvidia = Question::Conflict {
            incoming: "nvidia-470xx-utils".to_string(),
            removable: "nvidia-utils".to_string(),
        };
        let plan = Plan {
            conflicts: conflicting_report(),
            ..Default::default()
        };
        let review = Review {
            part1: vec![linux.clone()],
            ..Default::default()
        };
        assert_eq!(
            merge_plan_conflicts(review, Some(&plan)).part1,
            [
                linux.clone(),
                nvidia.clone(),
                Question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava".to_string(),
                },
                Question::Conflict {
                    incoming: "cava-git".to_string(),
                    removable: "cava-old".to_string(),
                },
            ]
        );
        let empty = Review::default();
        assert!(merge_plan_conflicts(empty, None).part1.is_empty());
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
        assert!(plan_conflicts_to_questions(&ConflictReport::default()).is_empty());
    }

    #[test]
    fn repo_target_names_pins_database_and_rejects_empty_rows() {
        let plan = Plan {
            repo_installs: vec![crate::resolve::RepoInstall {
                name: "neovim".to_string(),
                version: "0.10.0-1".to_string(),
                db: "extra".to_string(),
                make: false,
                target: true,
            }],
            ..Default::default()
        };
        assert_eq!(
            repo_target_names(Some(&plan)).unwrap(),
            vec!["extra/neovim=0.10.0-1".to_string()]
        );
        assert!(repo_target_names(None).unwrap().is_empty());
        let plan = Plan {
            repo_installs: vec![crate::resolve::RepoInstall {
                name: "neovim".to_string(),
                version: "0.10.0-1".to_string(),
                db: String::new(),
                make: false,
                target: true,
            }],
            ..Default::default()
        };
        let error = repo_target_names(Some(&plan)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("resolver produced no database for package neovim")
        );
    }

    fn engine_handle() -> (tempfile::TempDir, alpm::Alpm) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        let entries = [
            ("neovim", "1.0-1", ""),
            ("vim", "1.0-1", "%GROUPS%\neditors\n\n"),
            ("foo", "2.0-1", ""),
            ("provider", "1.0-1", "%PROVIDES%\nvirt\n\n"),
        ];
        let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, version, extra) in entries {
            let desc = format!(
                "%NAME%\n{name}\n\n%VERSION%\n{version}\n\n%FILENAME%\n{name}-{version}-x86_64.pkg.tar.zst\n\n{extra}"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(desc.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{name}-{version}/desc"),
                    desc.as_bytes(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        handle
            .register_syncdb_mut("core", alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
        (dir, handle)
    }

    fn peeled(names: &[&str], pure_repo: bool) -> PeeledTargets {
        PeeledTargets {
            files: Vec::new(),
            names: names.iter().map(|name| name.to_string()).collect(),
            pure_repo,
            reinstall: false,
        }
    }

    #[test]
    fn engine_routing_covers_repo_forms() {
        let (_dir, handle) = engine_handle();
        for target in ["neovim", "virt", "foo>=2", "core/neovim", "foo", "editors"] {
            assert!(routes_to_engine(&handle, target), "{target}");
        }
        assert!(!routes_to_engine(&handle, "yay-bin"));
    }

    #[test]
    fn group_expansion_applies_only_without_tty() {
        let (_dir, handle) = engine_handle();
        assert_eq!(
            expand_install_groups(&handle, &["editors".to_string()], true),
            vec!["editors".to_string()]
        );
        assert_eq!(
            expand_install_groups(&handle, &["editors".to_string()], false),
            vec!["vim".to_string()]
        );
        assert_eq!(
            expand_install_groups(&handle, &["yay-bin".to_string()], false),
            vec!["yay-bin".to_string()]
        );
    }

    #[test]
    fn pure_repo_decision_gates_direct_dispatch() {
        let direct = peeled(&["neovim", "virt"], true);
        assert_eq!(
            direct_install_targets(&direct),
            Some(vec!["neovim".to_string(), "virt".to_string()])
        );
        assert_eq!(
            direct_install_targets(&peeled(&["neovim", "yay-bin"], false)),
            None
        );
    }

    struct SecondProvider;

    impl AnswerSource for SecondProvider {
        fn answer(&self, question: &Question) -> crate::question::source::SourceDecision {
            let Question::SelectProvider { candidates, .. } = question else {
                return ExploreDefaults.answer(question);
            };
            let Some(second) = candidates.get(1) else {
                return ExploreDefaults.answer(question);
            };
            crate::question::source::SourceDecision::Answer(
                crate::question::model::Answer::SelectProvider {
                    name: second.name.clone(),
                    repo: second.repo.clone(),
                },
            )
        }
    }

    fn provider_preview_handle() -> (tempfile::TempDir, alpm::Alpm, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let db = dir.path().join("db");
        let stubs = dir.path().join("stubs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(db.join("local")).unwrap();
        std::fs::create_dir_all(db.join("sync")).unwrap();
        std::fs::create_dir_all(&stubs).unwrap();
        let file = std::fs::File::create(db.join("sync").join("core.db")).unwrap();
        let mut builder = tar::Builder::new(file);
        for name in ["provider-one", "provider-two"] {
            let desc = format!(
                "%NAME%\n{name}\n\n%VERSION%\n1.0-1\n\n%FILENAME%\n{name}-1.0-1-x86_64.pkg.tar.zst\n\n%PROVIDES%\nvirt\n\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(desc.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{name}-1.0-1/desc"), desc.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap();
        let mut handle = alpm::Alpm::new(
            root.to_string_lossy().as_ref(),
            db.to_string_lossy().as_ref(),
        )
        .unwrap();
        handle
            .register_syncdb_mut("core", alpm::SigLevel::NONE)
            .unwrap()
            .add_server("file:///pakajo-offline-stub")
            .unwrap();
        crate::tx::targets::write_cachedir_stub(
            &stubs,
            "needsvirt",
            "1.0-1",
            &crate::tx::targets::StubLists {
                depends: &["virt"],
                ..Default::default()
            },
        );
        let target = stubs
            .join(crate::tx::targets::filename("needsvirt", "1.0-1"))
            .to_string_lossy()
            .into_owned();
        (dir, handle, target)
    }

    fn preview_request(targets: &[&str]) -> InstallRequest {
        InstallRequest {
            targets: targets.iter().map(|target| target.to_string()).collect(),
            as_deps: false,
            reinstall: false,
            no_check: false,
            ignores: Vec::new(),
            prefer_aur: false,
            decider: Box::new(crate::dispatch::protocol::AutomaticDecider::new()),
            approvals: None,
            tty: false,
            json: false,
        }
    }

    fn preview_names(preview: &InstallPreview) -> Vec<String> {
        let mut names: Vec<String> = preview
            .review
            .part2
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn install_preview_with_honors_provider_source() {
        let (_dir, mut handle, target) = provider_preview_handle();
        let request = preview_request(&[&target]);
        let defaults = run_install_preview(&mut handle, &request).unwrap();
        assert!(preview_names(&defaults).contains(&"provider-one".to_string()));
        assert!(!preview_names(&defaults).contains(&"provider-two".to_string()));
        let second =
            run_install_preview_with(&mut handle, &request, Box::new(SecondProvider)).unwrap();
        assert!(preview_names(&second).contains(&"provider-two".to_string()));
        assert!(!preview_names(&second).contains(&"provider-one".to_string()));
    }
}
