use futures::SinkExt as _;

use crate::dispatch::approvals::ApprovalsFile;
use crate::dispatch::exec::{ChildOutcome, DispatchStream, StreamItem, send_done};
use crate::dispatch::operation::PrivilegedOperation;
use crate::dispatch::protocol::Decider;
use crate::dispatch::seal::{JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing};
use crate::dispatch::session::{
    AfterRepo, PhasePlan, RepoPhase, after_repo, repo_phase_from_child, repo_phase_from_result,
    run_phases,
};
use crate::events::InstallEvent;

pub struct SysupgradeRequest {
    pub no_refresh: bool,
    pub repo_only: bool,
    pub ignores: Vec<String>,
    pub decider: Box<dyn Decider + Send>,
    pub aur_targets: Option<Vec<String>>,
    pub approvals: Option<String>,
    pub tty: bool,
    pub json: bool,
    pub print_nothing_to_do: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NothingToDoSink {
    Stdout,
    Stderr,
}

pub fn nothing_to_do_sink(print_nothing_to_do: bool, json: bool) -> Option<NothingToDoSink> {
    if !print_nothing_to_do {
        return None;
    }
    if json {
        Some(NothingToDoSink::Stderr)
    } else {
        Some(NothingToDoSink::Stdout)
    }
}

pub fn sysupgrade(request: SysupgradeRequest) -> DispatchStream {
    let (tx, rx) = futures::channel::mpsc::channel(256);
    std::thread::spawn(move || run_sysupgrade(request, tx));
    rx
}

pub(crate) struct UpgradeReview {
    pub(crate) review: crate::question::review::Review,
    pub(crate) aur: Vec<crate::upgrade::AurUpgradeCandidate>,
}

pub(crate) fn upgrade_review(
    handle: &mut alpm::Alpm,
    source: Box<dyn crate::question::source::AnswerSource>,
) -> anyhow::Result<UpgradeReview> {
    let aur = preview_aur_candidates(handle);
    let outcome = crate::tx::compose::preview_with(handle, upgrade_run_spec(), source)?;
    if let crate::tx::driver::Finish::PrepareFailed(failure) = &outcome.finish {
        anyhow::bail!("upgrade preview failed to prepare: {failure}");
    }
    let review = outcome
        .review
        .ok_or_else(|| anyhow::anyhow!("upgrade explore run produced no review"))?;
    Ok(UpgradeReview { review, aur })
}

fn upgrade_run_spec() -> crate::tx::driver::RunSpec {
    crate::tx::driver::RunSpec {
        kind: crate::tx::driver::RunKind::Upgrade,
        targets: Vec::new(),
        stub_targets: Vec::new(),
        explore: true,
        as_deps: false,
        reinstall: false,
        dep_names: Vec::new(),
    }
}

fn preview_aur_candidates(handle: &alpm::Alpm) -> Vec<crate::upgrade::AurUpgradeCandidate> {
    match detect_aur_upgrades(handle) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!(
                "[pakajo] aur upgrade check failed, sysupgrade preview shows repo only: {error:#}"
            );
            Vec::new()
        }
    }
}

fn detect_aur_upgrades(
    handle: &alpm::Alpm,
) -> anyhow::Result<Vec<crate::upgrade::AurUpgradeCandidate>> {
    let aur = crate::aur::AurClient::new();
    let (candidates, _) =
        crate::upgrade::compute_aur_upgrades(handle, &aur, crate::upgrade::DevelSource::Live)?;
    let ignored: std::collections::HashSet<&str> = handle.ignorepkgs().iter().collect();
    Ok(candidates
        .into_iter()
        .filter(|candidate| !ignored.contains(candidate.name.as_str()))
        .collect())
}

pub(crate) const NON_INTERACTIVE_SEAL_REQUIRED: &str =
    "non-interactive sysupgrade requires sealed approvals";

fn run_sysupgrade(request: SysupgradeRequest, mut tx: futures::channel::mpsc::Sender<StreamItem>) {
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
    let interactive = request.tty && !request.json;
    if crate::cli::privs::is_root() {
        let phase = repo_phase_from_result(crate::dispatch::child::run_upgrade_repo_direct(
            request.no_refresh,
            &request.ignores,
            interactive,
            request.approvals.as_deref(),
            request.json,
        ));
        run_after_repo(request, phase, &mut tx);
        return;
    }
    let sealed = match request
        .approvals
        .as_deref()
        .map(|payload| ApprovalsFile::write(payload.as_bytes()))
        .transpose()
    {
        Ok(sealed) => sealed,
        Err(error) => {
            send_done(&mut tx, ChildOutcome::Failed(format!("{error:#}")));
            return;
        }
    };
    let Some(outcome) = run_repo_phase(&request, interactive, sealed, &mut tx) else {
        return;
    };
    run_after_repo(request, repo_phase_from_child(&outcome), &mut tx);
}

fn run_repo_phase(
    request: &SysupgradeRequest,
    interactive: bool,
    approvals: Option<ApprovalsFile>,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<ChildOutcome> {
    let operation = PrivilegedOperation::UpgradeRepo {
        no_refresh: request.no_refresh,
        ignores: request.ignores.clone(),
        interactive,
        approvals,
    };
    match crate::dispatch::session::forward(operation.dispatch(request.tty), tx) {
        crate::dispatch::session::ForwardEnd::Done(outcome) => Some(outcome),
        crate::dispatch::session::ForwardEnd::InnerEnded => {
            send_done(tx, ChildOutcome::Failed("stream ended".to_string()));
            None
        }
        crate::dispatch::session::ForwardEnd::ReceiverGone => None,
    }
}

fn run_after_repo(
    request: SysupgradeRequest,
    repo_phase: RepoPhase,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) {
    let Some(aur_targets) = detect_aur_targets(&request, tx) else {
        return;
    };
    match after_repo(&repo_phase, &aur_targets) {
        AfterRepo::BuildAur => {}
        AfterRepo::NothingToDo => {
            match nothing_to_do_sink(request.print_nothing_to_do, request.json) {
                Some(NothingToDoSink::Stderr) => eprintln!(" there is nothing to do"),
                Some(NothingToDoSink::Stdout) => println!(" there is nothing to do"),
                None => {}
            }
            send_done(tx, ChildOutcome::Success);
            return;
        }
        AfterRepo::Done(outcome) => {
            send_done(tx, outcome);
            return;
        }
    }
    run_phases(
        PhasePlan {
            privileged: None,
            repo_committed: matches!(repo_phase, RepoPhase::Committed),
            aur_targets,
            files: Vec::new(),
            as_deps: false,
            reinstall: false,
            no_check: false,
            repo_verb: "upgraded",
            approvals_payload: request.approvals,
            decider: request.decider,
            tty: request.tty,
            interactive: request.tty && !request.json,
        },
        tx,
    );
}

fn detect_aur_targets(
    request: &SysupgradeRequest,
    tx: &mut futures::channel::mpsc::Sender<StreamItem>,
) -> Option<Vec<String>> {
    if request.repo_only {
        return Some(Vec::new());
    }
    if let Some(targets) = &request.aur_targets {
        return Some(targets.clone());
    }
    let config = match crate::pacman::config() {
        Ok(config) => config,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return None;
        }
    };
    let mut handle = match crate::pacman::handle_with_config(&config) {
        Ok(handle) => handle,
        Err(error) => {
            send_done(tx, ChildOutcome::Failed(format!("{error:#}")));
            return None;
        }
    };
    crate::upgrade::apply_ignores(&mut handle, &config, &request.ignores);
    let mut candidates = match detect_aur_upgrades(&handle) {
        Ok(candidates) => candidates,
        Err(error) => {
            eprintln!("warning: AUR upgrade detection failed: {error:#}");
            Vec::new()
        }
    };
    drop(handle);
    candidates.retain(|candidate| !request.ignores.contains(&candidate.name));
    let targets: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.name.clone())
        .collect();
    if futures::executor::block_on(tx.send(StreamItem::Event(
        InstallEvent::SysupgradeAurCandidates { candidates },
    )))
    .is_err()
    {
        return None;
    }
    Some(targets)
}

#[cfg(test)]
mod tests {
    use super::{NON_INTERACTIVE_SEAL_REQUIRED, nothing_to_do_sink, sysupgrade};
    use super::{NothingToDoSink, SysupgradeRequest};
    use crate::dispatch::exec::{ChildOutcome, drain_declining};
    use crate::dispatch::seal::{
        JSON_SEAL_REQUIRED, json_seal_missing, non_interactive_seal_missing,
    };

    struct DiscardSink;

    impl crate::events::InstallSink for DiscardSink {
        fn event(&mut self, _event: crate::events::InstallEvent) {}
    }

    fn gate_request(json: bool, tty: bool, approvals: Option<&str>) -> SysupgradeRequest {
        SysupgradeRequest {
            no_refresh: true,
            repo_only: true,
            ignores: Vec::new(),
            decider: Box::new(crate::dispatch::protocol::AutomaticDecider::new()),
            aur_targets: Some(Vec::new()),
            approvals: approvals.map(str::to_string),
            tty,
            json,
            print_nothing_to_do: false,
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
            "non-interactive sysupgrade requires sealed approvals"
        );
    }

    #[test]
    fn json_without_seal_fails_closed() {
        let outcome = drain_declining(
            sysupgrade(gate_request(true, false, None)),
            &mut DiscardSink,
        );
        assert!(
            matches!(
                &outcome,
                ChildOutcome::Failed(message) if message.as_str() == JSON_SEAL_REQUIRED
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn non_interactive_without_seal_fails_closed() {
        let outcome = drain_declining(
            sysupgrade(gate_request(false, false, None)),
            &mut DiscardSink,
        );
        assert!(
            matches!(
                &outcome,
                ChildOutcome::Failed(message) if message.as_str() == NON_INTERACTIVE_SEAL_REQUIRED
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn nothing_to_do_prints_for_cli_callers_only() {
        assert_eq!(nothing_to_do_sink(false, false), None);
        assert_eq!(nothing_to_do_sink(false, true), None);
        assert_eq!(
            nothing_to_do_sink(true, false),
            Some(NothingToDoSink::Stdout)
        );
        assert_eq!(
            nothing_to_do_sink(true, true),
            Some(NothingToDoSink::Stderr)
        );
    }
}
