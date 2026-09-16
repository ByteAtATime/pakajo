use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::aur::AurClient;

use super::plan::{
    GroupMember, Missing, OpenQuestion, Plan, build_plan_from_plan, plan_from_actions,
};
use super::raur::{AurRaur, RaurError};
use super::sources::{AurQuery, PackageDb};
use super::types::BuildPlan;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAnswer {
    pub depend: String,
    pub provider_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupAnswer {
    pub group: String,
    pub member_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answers {
    pub providers: Vec<ProviderAnswer>,
    pub groups: Vec<GroupAnswer>,
}

pub enum Decisions {
    Default,
    Ask(Box<dyn Ask>),
    Pinned(Answers),
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
                Ok(plan_from_actions(&actions, vec![]))
            }
            Decisions::Ask(ask) => {
                let actions = self.resolve_interactive(targets, Rc::new(RefCell::new(ask)))?;
                Ok(plan_from_actions(&actions, vec![]))
            }
            Decisions::Pinned(answers) => {
                let state = Rc::new(RefCell::new(PinnedState {
                    answers,
                    questions: Vec::new(),
                }));
                let actions = self.resolve_interactive(targets, Rc::clone(&state))?;
                let questions = state.borrow().questions.clone();
                Ok(plan_from_actions(&actions, questions))
            }
        }
    }
}

struct PinnedState {
    answers: Answers,
    questions: Vec<OpenQuestion>,
}

impl Ask for PinnedState {
    fn choose_provider(&mut self, depend: &str, candidates: &[String]) -> usize {
        let pinned = self
            .answers
            .providers
            .iter()
            .find(|answer| answer.depend == depend)
            .and_then(|answer| {
                candidates
                    .iter()
                    .position(|name| *name == answer.provider_name)
            });
        if let Some(index) = pinned {
            return index;
        }
        self.questions.push(OpenQuestion::Provider {
            depend: depend.to_string(),
            candidates: candidates.to_vec(),
            chosen: candidates.first().cloned().unwrap_or_default(),
        });
        0
    }

    fn choose_group_members(&mut self, group: &str, members: &[GroupMember]) -> Vec<usize> {
        let pinned = self
            .answers
            .groups
            .iter()
            .find(|answer| answer.group == group)
            .map(|answer| {
                members
                    .iter()
                    .enumerate()
                    .filter(|(_, member)| answer.member_names.contains(&member.name))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>()
            })
            .filter(|matched| !matched.is_empty());
        if let Some(matched) = pinned {
            return matched;
        }
        self.questions.push(OpenQuestion::Group {
            group: group.to_string(),
            members: members.to_vec(),
            chosen: members.iter().map(|member| member.name.clone()).collect(),
        });
        (0..members.len()).collect()
    }
}

fn missing_message(missing: &Missing) -> String {
    if missing.stack.is_empty() {
        return format!("target not found in AUR: {}", missing.dep);
    }
    let chain = missing
        .stack
        .iter()
        .map(|frame| frame.pkg.as_str())
        .collect::<Vec<_>>()
        .join(" -> ");
    format!(
        "no AUR package found for {} (required by {chain})",
        missing.dep
    )
}

pub fn resolve(
    _db: &impl PackageDb,
    _aur: &impl AurQuery,
    targets: &[String],
    no_check: bool,
) -> anyhow::Result<BuildPlan> {
    if targets.is_empty() {
        return Ok(BuildPlan {
            targets: vec![],
            layers: vec![],
        });
    }
    let mut engine = Engine::new(no_check)?;
    let plan = engine
        .resolve(targets, Decisions::Default)
        .map_err(|error| anyhow::anyhow!("resolution failed: {error}"))?;
    if let Some(first) = plan.missing.first() {
        anyhow::bail!("{}", missing_message(first));
    }
    Ok(build_plan_from_plan(&plan, targets))
}
