use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;

use crate::aur::AurClient;

use super::plan::{GroupMember, Missing, Plan, plan_from_actions};
use super::raur::{AurRaur, RaurError};

#[derive(Debug)]
pub enum ResolveError {
    Alpm(alpm::Error),
    Raur(RaurError),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Alpm(error) => error.fmt(formatter),
            ResolveError::Raur(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ResolveError::Alpm(error) => Some(error),
            ResolveError::Raur(error) => Some(error),
        }
    }
}

impl From<aur_depends::Error> for ResolveError {
    fn from(error: aur_depends::Error) -> Self {
        match error {
            aur_depends::Error::Alpm(error) => ResolveError::Alpm(error),
            aur_depends::Error::Raur(error) => match error.downcast::<RaurError>() {
                Ok(raur) => ResolveError::Raur(*raur),
                Err(other) => ResolveError::Raur(RaurError::new(other.to_string(), Some(other))),
            },
        }
    }
}

fn extend_group<'a>(
    members: &mut Vec<GroupMember>,
    pkgs: &mut Vec<&'a alpm::Package>,
    db: &str,
    entry_pkgs: impl IntoIterator<Item = &'a alpm::Package>,
) {
    for pkg in entry_pkgs {
        members.push(GroupMember {
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
            db: db.to_string(),
        });
        pkgs.push(pkg);
    }
}

pub trait Ask: Send {
    fn choose_provider(&mut self, depend: &str, candidates: &[String]) -> usize;
    fn choose_group_members(&mut self, group: &str, members: &[GroupMember]) -> Vec<usize>;
}

impl Ask for Box<dyn Ask> {
    fn choose_provider(&mut self, depend: &str, candidates: &[String]) -> usize {
        (**self).choose_provider(depend, candidates)
    }

    fn choose_group_members(&mut self, group: &str, members: &[GroupMember]) -> Vec<usize> {
        (**self).choose_group_members(group, members)
    }
}

pub enum Decisions {
    Default,
    Ask(Box<dyn Ask>),
}

pub struct Engine {
    alpm: alpm::Alpm,
    raur: AurRaur,
    cache: raur::Cache,
    no_check: bool,
}

impl Engine {
    pub fn new(no_check: bool) -> anyhow::Result<Self> {
        let alpm = crate::pacman::handle().context("failed to open alpm handle")?;
        Ok(Self {
            alpm,
            raur: AurRaur(AurClient::new()),
            cache: raur::Cache::default(),
            no_check,
        })
    }

    fn flags(&self) -> aur_depends::Flags {
        if self.no_check {
            aur_depends::Flags::new() - aur_depends::Flags::CHECK_DEPENDS
        } else {
            aur_depends::Flags::new()
        }
    }

    fn resolver(&mut self, flags: aur_depends::Flags) -> aur_depends::Resolver<'_, '_, AurRaur> {
        let Self {
            alpm, cache, raur, ..
        } = &mut *self;
        aur_depends::Resolver::new(alpm, cache, raur, flags)
            .aur_namespace(true)
            .is_devel(|name| {
                ["-git", "-svn", "-hg", "-bzr", "-cvs", "-darcs"]
                    .iter()
                    .any(|suffix| name.ends_with(*suffix))
            })
    }

    fn resolve_interactive<S: Ask + 'static>(
        &mut self,
        targets: &[String],
        asker: Rc<RefCell<S>>,
    ) -> Result<aur_depends::Actions<'_>, ResolveError> {
        let provider_asker = Rc::clone(&asker);
        let group_asker = Rc::clone(&asker);
        Ok(futures::executor::block_on(
            self.resolver(self.flags())
                .provider_callback(move |depend, candidates| {
                    let owned: Vec<String> =
                        candidates.iter().map(|name| name.to_string()).collect();
                    provider_asker.borrow_mut().choose_provider(depend, &owned)
                })
                .group_callback(move |groups| {
                    let mut name = String::new();
                    let mut members = Vec::new();
                    let mut pkgs = Vec::new();
                    for entry in groups {
                        if name.is_empty() {
                            name = entry.group.name().to_string();
                        }
                        extend_group(
                            &mut members,
                            &mut pkgs,
                            entry.db.name(),
                            entry.group.packages(),
                        );
                    }
                    group_asker
                        .borrow_mut()
                        .choose_group_members(&name, &members)
                        .into_iter()
                        .map(|index| pkgs[index])
                        .collect()
                })
                .resolve_targets(targets),
        )?)
    }

    pub fn resolve(
        &mut self,
        targets: &[String],
        decisions: Decisions,
    ) -> Result<Plan, ResolveError> {
        match decisions {
            Decisions::Default => {
                let actions = futures::executor::block_on(
                    self.resolver(self.flags()).resolve_targets(targets),
                )?;
                Ok(plan_from_actions(&actions))
            }
            Decisions::Ask(ask) => {
                let actions = self.resolve_interactive(targets, Rc::new(RefCell::new(ask)))?;
                Ok(plan_from_actions(&actions))
            }
        }
    }
}

pub(crate) fn resolve_plan_raw(
    targets: &[String],
    no_check: bool,
    decisions: Decisions,
) -> anyhow::Result<Plan> {
    let mut engine = Engine::new(no_check)?;
    engine
        .resolve(targets, decisions)
        .map_err(|error| anyhow::anyhow!("resolution failed: {error}"))
}

pub(crate) fn check_plan_gates(plan: &Plan) -> anyhow::Result<()> {
    if !plan.duplicates.is_empty() {
        anyhow::bail!("{}", duplicate_message(&plan.duplicates));
    }
    if !plan.missing.is_empty() {
        anyhow::bail!("{}", missing_report(&plan.missing));
    }
    Ok(())
}

pub(crate) fn resolve_plan(
    targets: &[String],
    no_check: bool,
    decisions: Decisions,
) -> anyhow::Result<Plan> {
    let plan = resolve_plan_raw(targets, no_check, decisions)?;
    check_plan_gates(&plan)?;
    Ok(plan)
}

pub(crate) fn duplicate_message(duplicates: &[String]) -> String {
    format!("duplicate packages: {}", duplicates.join(" "))
}

pub(crate) fn missing_report(missing: &[Missing]) -> String {
    let mut report = String::from("could not find all required packages:");
    for entry in missing {
        if entry.stack.is_empty() {
            report.push_str(&format!("\n    {} (target)", entry.dep));
            continue;
        }
        let stack = entry
            .stack
            .iter()
            .map(|frame| match &frame.dep {
                Some(dep) => format!("{} ({dep})", frame.pkg),
                None => frame.pkg.clone(),
            })
            .collect::<Vec<_>>()
            .join(" -> ");
        report.push_str(&format!("\n    {} (wanted by: {stack})", entry.dep));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Missing, MissingStack};

    #[test]
    fn duplicate_message_joins_all_names_with_single_space() {
        assert_eq!(
            duplicate_message(&["yay-bin".to_string(), "yay-bin".to_string()]),
            "duplicate packages: yay-bin yay-bin"
        );
    }

    #[test]
    fn check_plan_gates_rejects_cross_source_duplicates() {
        let plan = crate::resolve::Plan {
            duplicates: vec!["xterm".to_string(), "xterm".to_string()],
            ..Default::default()
        };
        let error = check_plan_gates(&plan).expect_err("duplicates must bail");
        assert_eq!(format!("{error:#}"), "duplicate packages: xterm xterm");
    }

    #[test]
    fn check_plan_gates_accepts_clean_plan() {
        check_plan_gates(&crate::resolve::Plan::default()).expect("clean plan passes all gates");
    }

    #[test]
    fn missing_report_lists_every_entry_paru_verbatim() {
        let missing = vec![
            Missing {
                dep: "this-does-not-exist-xyz".to_string(),
                stack: Vec::new(),
            },
            Missing {
                dep: "libfoo".to_string(),
                stack: vec![
                    MissingStack {
                        pkg: "bar".to_string(),
                        dep: Some("libfoo".to_string()),
                    },
                    MissingStack {
                        pkg: "baz".to_string(),
                        dep: None,
                    },
                ],
            },
        ];
        assert_eq!(
            missing_report(&missing),
            "could not find all required packages:\n    this-does-not-exist-xyz (target)\n    libfoo (wanted by: bar (libfoo) -> baz)"
        );
    }
}
