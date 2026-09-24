use crate::question::model::Question;
use crate::question::{Conflict, ProviderPrompt, QuestionSet};

pub use crate::tx::convert::PrepareFailure;

pub fn question_set_from_review(review: &crate::question::review::Review) -> QuestionSet {
    let mut conflicts = Vec::new();
    let mut providers = Vec::new();
    let mut unsupported_summary = String::new();
    for question in &review.part1 {
        match question {
            Question::Conflict {
                incoming,
                removable,
            } => conflicts.push(Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
            }),
            Question::SelectProvider { depend, candidates } => {
                providers.push(ProviderPrompt {
                    depend: depend.clone(),
                    candidates: candidates.clone(),
                });
            }
            Question::Replace { .. } => unsupported_summary.push_str("replace; "),
            Question::InstallIgnorepkg { .. } => {
                unsupported_summary.push_str("install-ignorepkg; ");
            }
            Question::Corrupted { .. } => unsupported_summary.push_str("corrupted; "),
            Question::RemovePkgs { .. } => unsupported_summary.push_str("remove-pkgs; "),
            Question::ImportKey { .. } => unsupported_summary.push_str("import-key; "),
            Question::HoldPkgs { .. }
            | Question::Proceed { .. }
            | Question::GroupMembers { .. } => {}
        }
    }
    QuestionSet {
        conflicts,
        providers,
        had_unsupported_question: !unsupported_summary.is_empty(),
        unsupported_summary,
        held: Vec::new(),
    }
}

#[cfg(test)]
mod converter_tests {
    use super::question_set_from_review;
    use super::{Conflict, ProviderPrompt};
    use crate::question::model::{ProviderCandidate, Question, TransactionKind};
    use crate::question::review::Review;

    fn review_of(part1: Vec<Question>) -> Review {
        Review {
            part1,
            part2: crate::events::TransactionSummary::default(),
            generated_by: crate::question::review::ExploreStamp::default(),
        }
    }

    #[test]
    fn review_maps_conflicts_providers_and_unsupported() {
        let part1 = vec![
            Question::Conflict {
                incoming: "newpkg".to_string(),
                removable: "oldpkg".to_string(),
            },
            Question::SelectProvider {
                depend: "virt".to_string(),
                candidates: vec![ProviderCandidate {
                    name: "provider-one".to_string(),
                    repo: Some("core".to_string()),
                    version: Some("1.0-1".to_string()),
                }],
            },
            Question::Replace {
                old: "nginx".to_string(),
                new: "nginx-mainline".to_string(),
                repo: None,
            },
            Question::Proceed {
                summary: crate::events::TransactionSummary::default(),
                kind: TransactionKind::Install,
            },
        ];
        let got = question_set_from_review(&review_of(part1));
        assert_eq!(
            got.conflicts,
            vec![Conflict {
                incoming: "newpkg".to_string(),
                removable: "oldpkg".to_string(),
            }]
        );
        assert_eq!(
            got.providers,
            vec![ProviderPrompt {
                depend: "virt".to_string(),
                candidates: vec![ProviderCandidate {
                    name: "provider-one".to_string(),
                    repo: Some("core".to_string()),
                    version: Some("1.0-1".to_string()),
                }],
            }]
        );
        assert!(got.had_unsupported_question);
        assert_eq!(got.unsupported_summary, "replace; ");
        assert!(got.held.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use crate::updates::compute_repo_upgrades;

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    const ROOT_DB_LCK: &str = "/var/lib/pacman/db.lck";
    const ROOT_SYNC_DIR: &str = "/var/lib/pacman/sync";

    fn sync_db_mtimes() -> BTreeMap<String, SystemTime> {
        let mut map = BTreeMap::new();
        let Ok(entries) = fs::read_dir(ROOT_SYNC_DIR) else {
            return map;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !(name.ends_with(".db") || name.ends_with(".files")) {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && let Ok(mtime) = meta.modified()
            {
                map.insert(name, mtime);
            }
        }
        map
    }

    #[test]
    #[ignore = "integration: needs live pacman sync DBs + user checkdb; run with --ignored rootless_sysupgrade"]
    fn rootless_sysupgrade_preview_matches_pacman_qu() {
        let root_lock = Path::new(ROOT_DB_LCK);
        assert!(
            !root_lock.exists(),
            "pre-existing {ROOT_DB_LCK} blocks a clean spike; remove it first"
        );
        let mtimes_before = sync_db_mtimes();

        let config = crate::pacman::config().expect("pacman config");
        let checkdb_lock = crate::utils::cache_root()
            .expect("cache root")
            .join("checkdb")
            .join("db.lck");

        let mut handle = crate::pacman::handle_rootless_with_config(&config)
            .expect("failed to build rootless alpm handle (Phase 1.1 primitive)");
        handle
            .syncdbs_mut()
            .update(false)
            .expect("failed to refresh checkdb sync DBs rootless");

        let manual: BTreeSet<String> = compute_repo_upgrades(&handle, &config)
            .expect("failed to compute manual repo upgrades")
            .iter()
            .map(|u| u.name.clone())
            .collect();

        crate::upgrade::apply_ignores(&mut handle, &config, &[]);
        handle
            .trans_init(alpm::TransFlag::DB_ONLY | alpm::TransFlag::NO_LOCK)
            .expect("failed to init sysupgrade dry-run transaction");
        handle
            .sync_sysupgrade(false)
            .expect("sync_sysupgrade failed to resolve upgrade targets");
        let spike: BTreeSet<String> = handle
            .trans_add()
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        let _ = handle.trans_release();

        assert!(
            !root_lock.exists(),
            "NOLOCK invariant violated: {ROOT_DB_LCK} was created"
        );
        assert!(
            !checkdb_lock.exists(),
            "NOLOCK invariant violated: checkdb db.lck was created"
        );
        assert_eq!(
            sync_db_mtimes(),
            mtimes_before,
            "no-write invariant violated: live sync DB mtimes changed"
        );

        let missing: Vec<String> = manual.difference(&spike).cloned().collect();
        assert!(
            missing.is_empty(),
            "every upgrade the manual vercmp sees (same checkdb data) must also be resolved by \
             rootless sync_sysupgrade. missing from spike: {missing:?}"
        );

        let spec = crate::tx::driver::RunSpec {
            kind: crate::tx::driver::RunKind::Upgrade,
            targets: Vec::new(),
            stub_targets: Vec::new(),
            explore: true,
            as_deps: false,
            reinstall: false,
            dep_names: Vec::new(),
        };
        let outcome =
            crate::tx::compose::preview(&mut handle, spec).expect("preview should succeed");
        let review = outcome.review.as_ref().expect("explore carries a review");
        let questions = crate::dry_run::question_set_from_review(review);
        let prepare_error = match outcome.finish {
            crate::tx::driver::Finish::PrepareFailed(failure) => Some(failure),
            crate::tx::driver::Finish::Stopped | crate::tx::driver::Finish::Committed => None,
        };
        assert!(
            !outcome.summary.packages.is_empty(),
            "preview summary must list the direct upgrade set"
        );
        eprintln!(
            "[preview] upgrades={} conflicts={} providers={} prepare_error={:?}",
            outcome.summary.packages.len(),
            questions.conflicts.len(),
            questions.providers.len(),
            prepare_error,
        );
        for pkg in outcome.summary.packages.iter().take(3) {
            eprintln!(
                "[preview] {} {} -> {}",
                pkg.name,
                pkg.old_version.as_deref().unwrap_or("-"),
                pkg.new_version,
            );
        }
    }
}
