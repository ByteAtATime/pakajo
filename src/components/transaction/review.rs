use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, button, checkbox, container, dialog, radio, scrollable, text};

use pakajo::progress::InstallKind;
use pakajo::question::approvals::{SealedApprovals, seal as seal_answers};
use pakajo::question::model::{Answer, Question, QuestionKey, TransactionKind};
use pakajo::question::revalidate::ReviewDrift;
use pakajo::question::{ProviderCandidate, QuestionSet};

use super::TransactionMessage;
use crate::Element;

#[derive(Clone, Debug)]
pub enum ReviewMessage {
    ToggleConflict(usize),
    ToggleReplace(usize),
    ToggleIgnorepkg(usize),
    ToggleRemovepkgs(usize),
    ToggleHoldpkgs(usize),
    SelectProvider { depend: String, idx: usize },
}

#[derive(Clone, Debug)]
pub(crate) enum ReviewSelection {
    ToggleConflict(usize),
    SelectProvider { depend: String, idx: usize },
}

pub(crate) fn review_body<'a>(
    qs: &'a QuestionSet,
    checks: &'a [bool],
    choices: &'a HashMap<String, usize>,
) -> cosmic::Element<'a, ReviewSelection> {
    let mut body = Column::new().spacing(16);
    if !qs.held.is_empty() {
        body = body.push(held_block(&qs.held));
    }
    if !qs.conflicts.is_empty() {
        body = body.push(text("Conflicts"));
        for (i, conflict) in qs.conflicts.iter().enumerate() {
            let label = format!("Replace {} with {}", conflict.removable, conflict.incoming);
            let checked = checks.get(i).copied().unwrap_or(false);
            let item = checkbox(checked)
                .label(label)
                .on_toggle(move |_| ReviewSelection::ToggleConflict(i));
            body = body.push(item);
        }
    }
    if !qs.providers.is_empty() {
        body = body.push(text("Providers"));
        for prompt in &qs.providers {
            body = body.push(text(prompt.depend.clone()));
            let selected = choices.get(&prompt.depend).copied().unwrap_or(0);
            for (idx, candidate) in prompt.candidates.iter().enumerate() {
                let label = candidate_label(candidate);
                let depend = prompt.depend.clone();
                let item = radio(text(label), idx, Some(selected), move |chosen: usize| {
                    ReviewSelection::SelectProvider {
                        depend: depend.clone(),
                        idx: chosen,
                    }
                });
                body = body.push(item);
            }
        }
    }
    if qs.had_unsupported_question {
        body = body.push(unsupported_banner(&qs.unsupported_summary));
    }
    body.into()
}

pub(crate) struct ReviewModel {
    pub(crate) qs: QuestionSet,
    pub(crate) conflict_checks: Vec<bool>,
    pub(crate) provider_choices: HashMap<String, usize>,
}

impl ReviewModel {
    pub(crate) fn new(qs: QuestionSet) -> Self {
        let conflict_checks = vec![true; qs.conflicts.len()];
        let provider_choices = qs
            .providers
            .iter()
            .map(|prompt| (prompt.depend.clone(), 0))
            .collect();
        Self {
            qs,
            conflict_checks,
            provider_choices,
        }
    }

    pub(crate) fn update(&mut self, message: ReviewMessage) {
        match message {
            ReviewMessage::ToggleConflict(i) => {
                if let Some(check) = self.conflict_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::ToggleReplace(_)
            | ReviewMessage::ToggleIgnorepkg(_)
            | ReviewMessage::ToggleRemovepkgs(_)
            | ReviewMessage::ToggleHoldpkgs(_) => {}
            ReviewMessage::SelectProvider { depend, idx } => {
                self.provider_choices.insert(depend, idx);
            }
        }
    }

    pub(crate) fn toggle_conflict(&mut self, i: usize) {
        self.update(ReviewMessage::ToggleConflict(i));
    }

    pub(crate) fn select_provider(&mut self, depend: String, idx: usize) {
        self.update(ReviewMessage::SelectProvider { depend, idx });
    }
}

fn candidate_label(candidate: &ProviderCandidate) -> String {
    let qualified = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    match &candidate.version {
        Some(version) => format!("{qualified}  {version}"),
        None => qualified,
    }
}

fn held_block(names: &[String]) -> cosmic::Element<'static, ReviewSelection> {
    let mut content = Column::new().spacing(12);
    let mut list = Column::new().spacing(8);
    for held in names {
        list = list.push(text::monotext(held.clone()).font(cosmic::font::bold()));
    }
    content = content.push(list);
    content
        .push(text::body(
            "This package is protected by your system configuration (HeldPkgs) because it is critical to your system.",
        ))
        .push(text::body(
            "Removing it may break your system completely. Unless you are sure you know what you are doing, this is probably not what you want.",
        ))
        .into()
}

fn unsupported_banner(summary: &str) -> cosmic::Element<'_, ReviewSelection> {
    container(text(summary.to_string()))
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            text_color: Some(Color::from(theme.cosmic().warning.on)),
            background: Some(Background::Color(Color::from(theme.cosmic().warning.base))),
            border: Border {
                radius: theme.cosmic().corner_radii.radius_s.into(),
                width: 1.0,
                color: Color::from(theme.cosmic().warning.base),
            },
            ..Default::default()
        })
        .into()
}

pub(crate) struct InstallReview {
    pub(crate) questions: Vec<Question>,
    pub(crate) conflict_checks: Vec<bool>,
    pub(crate) replace_checks: Vec<bool>,
    pub(crate) ignorepkg_checks: Vec<bool>,
    pub(crate) removepkgs_checks: Vec<bool>,
    pub(crate) holdpkgs_checks: Vec<bool>,
    pub(crate) provider_choices: HashMap<String, usize>,
    pub(crate) highlighted: BTreeSet<QuestionKey>,
    pub(crate) added: BTreeSet<QuestionKey>,
    pub(super) approving: bool,
}

impl InstallReview {
    pub(crate) fn new(questions: Vec<Question>) -> Self {
        let mut seen = HashSet::new();
        let questions: Vec<Question> = questions
            .into_iter()
            .filter(|question| seen.insert(question.key()))
            .collect();
        Self {
            conflict_checks: vec![true; questions.len()],
            replace_checks: questions
                .iter()
                .map(|question| matches!(question, Question::Replace { .. }))
                .collect(),
            ignorepkg_checks: vec![false; questions.len()],
            removepkgs_checks: vec![true; questions.len()],
            holdpkgs_checks: vec![false; questions.len()],
            provider_choices: questions
                .iter()
                .filter_map(|question| match question {
                    Question::SelectProvider { depend, .. } => Some((depend.clone(), 0)),
                    _ => None,
                })
                .collect(),
            questions,
            highlighted: BTreeSet::new(),
            added: BTreeSet::new(),
            approving: false,
        }
    }

    pub(crate) fn refresh(&mut self, questions: Vec<Question>, drift: &ReviewDrift) {
        let mut preserved: BTreeMap<QuestionKey, (Question, Answer)> = BTreeMap::new();
        for (i, old) in self.questions.iter().enumerate() {
            preserved.insert(old.key(), (old.clone(), self.answer_for(i, old)));
        }
        let mut seen = HashSet::new();
        let fresh: Vec<Question> = questions
            .into_iter()
            .filter(|question| seen.insert(question.key()))
            .collect();
        let mut conflict_checks = Vec::with_capacity(fresh.len());
        let mut replace_checks = Vec::with_capacity(fresh.len());
        let mut ignorepkg_checks = Vec::with_capacity(fresh.len());
        let mut removepkgs_checks = Vec::with_capacity(fresh.len());
        let mut holdpkgs_checks = Vec::with_capacity(fresh.len());
        let mut provider_choices = HashMap::with_capacity(fresh.len());
        for question in &fresh {
            let restored = preserved
                .get(&question.key())
                .filter(|(old, _)| old == question)
                .map(|(_, answer)| answer);
            conflict_checks.push(restored_conflict(question, restored).unwrap_or(true));
            replace_checks.push(
                restored_replace(question, restored)
                    .unwrap_or(matches!(question, Question::Replace { .. })),
            );
            ignorepkg_checks.push(restored_ignorepkg(question, restored).unwrap_or(false));
            removepkgs_checks.push(restored_removepkgs(question, restored).unwrap_or(true));
            holdpkgs_checks.push(restored_holdpkgs(question, restored).unwrap_or(false));
            if let Question::SelectProvider { depend, candidates } = question {
                provider_choices.insert(
                    depend.clone(),
                    restored_provider(candidates, restored).unwrap_or(0),
                );
            }
        }
        self.questions = fresh;
        self.conflict_checks = conflict_checks;
        self.replace_checks = replace_checks;
        self.ignorepkg_checks = ignorepkg_checks;
        self.removepkgs_checks = removepkgs_checks;
        self.holdpkgs_checks = holdpkgs_checks;
        self.provider_choices = provider_choices;
        self.highlighted = drift.added.union(&drift.changed).cloned().collect();
        self.added = drift.added.clone();
        self.approving = false;
    }

    pub(crate) fn update(&mut self, message: ReviewMessage) {
        match message {
            ReviewMessage::ToggleConflict(i) => {
                if let Some(check) = self.conflict_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::ToggleReplace(i) => {
                if let Some(check) = self.replace_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::ToggleIgnorepkg(i) => {
                if let Some(check) = self.ignorepkg_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::ToggleRemovepkgs(i) => {
                if let Some(check) = self.removepkgs_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::ToggleHoldpkgs(i) => {
                if let Some(check) = self.holdpkgs_checks.get_mut(i) {
                    *check = !*check;
                }
            }
            ReviewMessage::SelectProvider { depend, idx } => {
                self.provider_choices.insert(depend, idx);
            }
        }
    }

    fn answer_for(&self, i: usize, question: &Question) -> Answer {
        match question {
            Question::Conflict {
                incoming,
                removable,
            } => Answer::Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
                remove: self.conflict_checks.get(i).copied().unwrap_or(true),
            },
            Question::SelectProvider { depend, candidates } => {
                let chosen = self
                    .provider_choices
                    .get(depend)
                    .copied()
                    .and_then(|idx| candidates.get(idx))
                    .or_else(|| candidates.first());
                let (name, repo) = chosen
                    .map(|candidate| (candidate.name.clone(), candidate.repo.clone()))
                    .unwrap_or_default();
                Answer::SelectProvider { name, repo }
            }
            Question::Replace { old, new, .. } => Answer::Replace {
                old: old.clone(),
                new: new.clone(),
                replace: self.replace_checks.get(i).copied().unwrap_or(true),
            },
            Question::InstallIgnorepkg { name } => Answer::InstallIgnorepkg {
                name: name.clone(),
                install: self.ignorepkg_checks.get(i).copied().unwrap_or(false),
            },
            Question::RemovePkgs { names, .. } => Answer::RemovePkgs {
                names: names.clone(),
                skip: self.removepkgs_checks.get(i).copied().unwrap_or(true),
            },
            Question::HoldPkgs { names } => Answer::HoldPkgs {
                names: names.clone(),
                proceed: self.holdpkgs_checks.get(i).copied().unwrap_or(false),
            },
            Question::GroupMembers { members, .. } => Answer::GroupMembers {
                selected: members.clone(),
            },
            Question::Proceed { .. } => Answer::Proceed,
            Question::Corrupted { path } => Answer::Corrupted {
                path: path.clone(),
                remove: false,
            },
            Question::ImportKey { fingerprint, .. } => Answer::ImportKey {
                fingerprint: fingerprint.clone(),
                import: false,
            },
        }
    }

    pub(crate) fn answers(&self) -> Vec<Answer> {
        self.questions
            .iter()
            .enumerate()
            .map(|(i, question)| self.answer_for(i, question))
            .collect()
    }

    pub(crate) fn seal(&self) -> anyhow::Result<SealedApprovals> {
        seal_answers(&self.questions, &self.answers(), true)
    }

    pub(crate) fn can_confirm(&self) -> bool {
        self.questions
            .iter()
            .enumerate()
            .all(|(i, question)| match question {
                Question::HoldPkgs { .. } => self.holdpkgs_checks.get(i).copied().unwrap_or(false),
                Question::RemovePkgs { .. } => {
                    self.removepkgs_checks.get(i).copied().unwrap_or(false)
                }
                _ => true,
            })
    }

    pub(crate) fn view(&self, name: &str, kind: InstallKind) -> Element<'_> {
        let body = part1_body(RowInputs {
            questions: &self.questions,
            checks: &self.conflict_checks,
            replace_checks: &self.replace_checks,
            ignorepkg_checks: &self.ignorepkg_checks,
            removepkgs_checks: &self.removepkgs_checks,
            holdpkgs_checks: &self.holdpkgs_checks,
            choices: &self.provider_choices,
            highlighted: &self.highlighted,
            added: &self.added,
        })
        .map(|message| crate::Message::Transaction(TransactionMessage::Review(message)));
        let approve = crate::Message::Transaction(TransactionMessage::ApproveReview);
        let gated = self.can_confirm();
        let confirm: Element<'_> = if self.approving {
            button::suggested("Loading...").into()
        } else if matches!(kind, InstallKind::Remove) {
            let action = button::destructive(install_confirm_label(kind));
            if gated {
                action.on_press(approve).into()
            } else {
                action.into()
            }
        } else {
            let action = button::suggested(install_confirm_label(kind));
            if gated {
                action.on_press(approve).into()
            } else {
                action.into()
            }
        };
        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelReview,
        ));
        dialog()
            .title(install_review_title(name, kind))
            .control(scrollable(body).height(Length::Fixed(400.0)))
            .primary_action(confirm)
            .secondary_action(cancel)
            .into()
    }
}

pub(crate) fn install_needs_review(part1: &[Question]) -> bool {
    !part1.is_empty()
}

fn info_caption(question: &Question) -> Option<String> {
    match question {
        Question::GroupMembers { group, members } => Some(format!(
            "Group {group} was pre-expanded into its members: {}",
            members.join(", ")
        )),
        _ => None,
    }
}

fn restored_conflict(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::Conflict { .. }, Some(Answer::Conflict { remove, .. })) => Some(*remove),
        _ => None,
    }
}

fn restored_replace(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::Replace { .. }, Some(Answer::Replace { replace, .. })) => Some(*replace),
        _ => None,
    }
}

fn restored_ignorepkg(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::InstallIgnorepkg { .. }, Some(Answer::InstallIgnorepkg { install, .. })) => {
            Some(*install)
        }
        _ => None,
    }
}

fn restored_removepkgs(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::RemovePkgs { .. }, Some(Answer::RemovePkgs { skip, .. })) => Some(*skip),
        _ => None,
    }
}

fn restored_holdpkgs(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::HoldPkgs { .. }, Some(Answer::HoldPkgs { proceed, .. })) => Some(*proceed),
        _ => None,
    }
}

fn install_review_title(name: &str, kind: InstallKind) -> String {
    match kind {
        InstallKind::Remove => format!("Review removal of {name}"),
        InstallKind::Install | InstallKind::Upgrade => format!("Review installation of {name}"),
    }
}

fn install_confirm_label(kind: InstallKind) -> &'static str {
    match kind {
        InstallKind::Remove => "Confirm removal",
        InstallKind::Install | InstallKind::Upgrade => "Confirm",
    }
}

fn removepkgs_label(names: &[String], kind: &TransactionKind) -> String {
    match kind {
        TransactionKind::Remove => format!(
            "Skip missing packages and continue without them: {}",
            names.join(", ")
        ),
        TransactionKind::Install => format!(
            "Skip unresolvable packages and continue without them: {}",
            names.join(", ")
        ),
    }
}

fn holdpkgs_label(names: &[String]) -> String {
    format!("Remove held packages anyway: {}", names.join(", "))
}

fn restored_provider(
    candidates: &[pakajo::question::model::ProviderCandidate],
    restored: Option<&Answer>,
) -> Option<usize> {
    let Answer::SelectProvider { name, repo } = restored? else {
        return None;
    };
    Some(
        candidates
            .iter()
            .position(|candidate| &candidate.name == name && &candidate.repo == repo)
            .unwrap_or(0),
    )
}

fn highlight_caption<'a>(
    key: &QuestionKey,
    highlighted: &'a BTreeSet<QuestionKey>,
    added: &'a BTreeSet<QuestionKey>,
) -> Option<&'a str> {
    if !highlighted.contains(key) {
        return None;
    }
    if added.contains(key) {
        return Some("New");
    }
    Some("Changed")
}

fn highlight_wrap<'a>(
    row: cosmic::Element<'a, ReviewMessage>,
    caption: &str,
) -> cosmic::Element<'a, ReviewMessage> {
    let content = Column::new()
        .spacing(4)
        .push(text(caption.to_string()))
        .push(row);
    container(content)
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            text_color: Some(Color::from(theme.cosmic().warning.on)),
            background: Some(Background::Color(Color::from(theme.cosmic().warning.base))),
            border: Border {
                radius: theme.cosmic().corner_radii.radius_s.into(),
                width: 1.0,
                color: Color::from(theme.cosmic().warning.base),
            },
            ..Default::default()
        })
        .into()
}

struct RowInputs<'a> {
    questions: &'a [Question],
    checks: &'a [bool],
    replace_checks: &'a [bool],
    ignorepkg_checks: &'a [bool],
    removepkgs_checks: &'a [bool],
    holdpkgs_checks: &'a [bool],
    choices: &'a HashMap<String, usize>,
    highlighted: &'a BTreeSet<QuestionKey>,
    added: &'a BTreeSet<QuestionKey>,
}

fn part1_body<'a>(inputs: RowInputs<'a>) -> cosmic::Element<'a, ReviewMessage> {
    let RowInputs {
        questions,
        checks,
        replace_checks,
        ignorepkg_checks,
        removepkgs_checks,
        holdpkgs_checks,
        choices,
        highlighted,
        added,
    } = inputs;
    let mut body = Column::new().spacing(16);
    let mut in_conflicts = false;
    let mut in_providers = false;
    for (i, question) in questions.iter().enumerate() {
        let caption = highlight_caption(&question.key(), highlighted, added);
        match question {
            Question::Conflict {
                incoming,
                removable,
            } => {
                if !in_conflicts {
                    body = body.push(text("Conflicts"));
                    in_conflicts = true;
                }
                let label = format!("Replace {} with {}", removable, incoming);
                let checked = checks.get(i).copied().unwrap_or(false);
                let row: cosmic::Element<'a, ReviewMessage> = checkbox(checked)
                    .label(label)
                    .on_toggle(move |_| ReviewMessage::ToggleConflict(i))
                    .into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            Question::SelectProvider { depend, candidates } => {
                if !in_providers {
                    body = body.push(text("Providers"));
                    in_providers = true;
                }
                let mut group = Column::new().spacing(8);
                group = group.push(text(depend.clone()));
                let selected = choices.get(depend).copied().unwrap_or(0);
                for (idx, candidate) in candidates.iter().enumerate() {
                    let label = candidate_label(candidate);
                    let depend = depend.clone();
                    group = group.push(radio(
                        text(label),
                        idx,
                        Some(selected),
                        move |chosen: usize| ReviewMessage::SelectProvider {
                            depend: depend.clone(),
                            idx: chosen,
                        },
                    ));
                }
                let row: cosmic::Element<'a, ReviewMessage> = group.into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            Question::Replace { old, new, .. } => {
                let label = format!("Replace {old} with {new}");
                let checked = replace_checks.get(i).copied().unwrap_or(true);
                let row: cosmic::Element<'a, ReviewMessage> = checkbox(checked)
                    .label(label)
                    .on_toggle(move |_| ReviewMessage::ToggleReplace(i))
                    .into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            Question::InstallIgnorepkg { name } => {
                let label = format!("Install {name} anyway (in IgnorePkg)");
                let checked = ignorepkg_checks.get(i).copied().unwrap_or(false);
                let row: cosmic::Element<'a, ReviewMessage> = checkbox(checked)
                    .label(label)
                    .on_toggle(move |_| ReviewMessage::ToggleIgnorepkg(i))
                    .into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            Question::RemovePkgs { names, kind } => {
                let label = removepkgs_label(names, kind);
                let checked = removepkgs_checks.get(i).copied().unwrap_or(true);
                let row: cosmic::Element<'a, ReviewMessage> = checkbox(checked)
                    .label(label)
                    .on_toggle(move |_| ReviewMessage::ToggleRemovepkgs(i))
                    .into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            Question::HoldPkgs { names } => {
                let label = holdpkgs_label(names);
                let checked = holdpkgs_checks.get(i).copied().unwrap_or(false);
                let mut group = Column::new().spacing(8);
                group = group.push(
                    checkbox(checked)
                        .label(label)
                        .on_toggle(move |_| ReviewMessage::ToggleHoldpkgs(i)),
                );
                group = group.push(text::body(
                    "These packages are protected by HoldPkg config on purpose because they are critical to your system.",
                ));
                group = group.push(text::body(
                    "Removing them can break your system. Unless you are sure you know what you are doing, this is probably not what you want.",
                ));
                let row: cosmic::Element<'a, ReviewMessage> = group.into();
                body = body.push(match caption {
                    Some(note) => highlight_wrap(row, note),
                    None => row,
                });
            }
            other => {
                if let Some(note) = info_caption(other) {
                    let row: cosmic::Element<'a, ReviewMessage> = text(note).into();
                    body = body.push(match caption {
                        Some(flag) => highlight_wrap(row, flag),
                        None => row,
                    });
                } else if let Some(flag) = caption {
                    let key = question.key();
                    let row: cosmic::Element<'a, ReviewMessage> = text(format!("{key:?}")).into();
                    body = body.push(highlight_wrap(row, flag));
                }
            }
        }
    }
    body.into()
}

#[cfg(test)]
mod install_review_tests {
    use super::*;
    use pakajo::question::model::{Answer, ProviderCandidate, QuestionKey, TransactionKind};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn candidate(name: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: s(name),
            repo: Some(s("extra")),
            version: None,
        }
    }

    fn questions() -> Vec<Question> {
        vec![
            Question::Conflict {
                incoming: s("cava-git"),
                removable: s("cava"),
            },
            Question::SelectProvider {
                depend: s("virt"),
                candidates: vec![candidate("qemu"), candidate("virtualbox")],
            },
            Question::Replace {
                old: s("gcc-multilib"),
                new: s("gcc"),
                repo: None,
            },
            Question::InstallIgnorepkg { name: s("glibc") },
            Question::RemovePkgs {
                names: vec![s("nvidia-utils")],
                kind: TransactionKind::Install,
            },
        ]
    }

    fn model_with(check: bool, provider_idx: usize) -> InstallReview {
        let mut model = InstallReview::new(questions());
        model.conflict_checks[0] = check;
        model.provider_choices.insert(s("virt"), provider_idx);
        model
    }

    #[test]
    fn seal_binds_choices_and_displayed_defaults() {
        let sealed = model_with(true, 1).seal().expect("seal succeeds");
        assert!(sealed.proceed);
        assert_eq!(sealed.answers.len(), 5);
        for (key, answer) in &sealed.answers {
            match (key, answer) {
                (QuestionKey::Conflict { .. }, Answer::Conflict { remove, .. }) => {
                    assert!(*remove)
                }
                (QuestionKey::SelectProvider { .. }, Answer::SelectProvider { name, repo }) => {
                    assert_eq!(
                        (name.as_str(), repo.as_deref()),
                        ("virtualbox", Some("extra"))
                    )
                }
                (QuestionKey::Replace { .. }, Answer::Replace { replace, .. }) => {
                    assert!(*replace)
                }
                (
                    QuestionKey::InstallIgnorepkg { .. },
                    Answer::InstallIgnorepkg { install, .. },
                ) => assert!(!*install),
                (QuestionKey::RemovePkgs { .. }, Answer::RemovePkgs { skip, .. }) => {
                    assert!(*skip)
                }
                other => panic!("unexpected sealed entry: {other:?}"),
            }
        }
    }

    #[test]
    fn seal_payload_decodes_back_with_choices_and_proceed() {
        let sealed = model_with(true, 1).seal().expect("seal succeeds");
        let payload = pakajo::dispatch::seal::encode_seal(&sealed).expect("encodes");
        let decoded = pakajo::dispatch::seal::decode_seal(&payload).expect("decodes");
        assert_eq!(decoded, sealed);
        assert!(decoded.proceed);
        assert_eq!(decoded.answers.len(), 5);
        assert!(decoded.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Conflict { .. },
                Answer::Conflict { remove: true, .. }
            )
        )));
        assert!(decoded.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::SelectProvider { .. },
                Answer::SelectProvider { .. }
            )
        )));
        assert!(decoded.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Replace { .. },
                Answer::Replace { replace: true, .. }
            )
        )));
        assert!(decoded.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::InstallIgnorepkg { .. },
                Answer::InstallIgnorepkg { install: false, .. }
            )
        )));
        assert!(decoded.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::RemovePkgs { .. },
                Answer::RemovePkgs { skip: true, .. }
            )
        )));
    }

    #[test]
    fn fresh_model_defaults_three_kinds() {
        let model = InstallReview::new(questions());
        assert!(model.replace_checks[2]);
        assert!(!model.ignorepkg_checks[3]);
        assert!(model.removepkgs_checks[4]);

        let sealed = model.seal().expect("seal succeeds");
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Replace { .. },
                Answer::Replace { replace: true, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::InstallIgnorepkg { .. },
                Answer::InstallIgnorepkg { install: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::RemovePkgs { .. },
                Answer::RemovePkgs { skip: true, .. }
            )
        )));
    }

    #[test]
    fn toggles_flip_three_kinds() {
        let mut model = InstallReview::new(questions());
        model.update(ReviewMessage::ToggleReplace(2));
        model.update(ReviewMessage::ToggleIgnorepkg(3));
        model.update(ReviewMessage::ToggleRemovepkgs(4));
        let sealed = model.seal().expect("seal succeeds");
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Replace { .. },
                Answer::Replace { replace: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::InstallIgnorepkg { .. },
                Answer::InstallIgnorepkg { install: true, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::RemovePkgs { .. },
                Answer::RemovePkgs { skip: false, .. }
            )
        )));
    }

    #[test]
    fn conflicts_default_to_removal() {
        let model = InstallReview::new(questions());
        assert!(model.conflict_checks.iter().all(|checked| *checked));

        let sealed = model.seal().expect("seal succeeds");
        let conflict = sealed
            .answers
            .iter()
            .find_map(|(key, answer)| match (key, answer) {
                (
                    QuestionKey::Conflict { .. },
                    Answer::Conflict {
                        incoming,
                        removable,
                        remove,
                    },
                ) => Some((incoming.clone(), removable.clone(), *remove)),
                _ => None,
            });
        assert_eq!(
            conflict,
            Some((s("cava-git"), s("cava"), true)),
            "conflict is sealed as remove: true"
        );
    }

    #[test]
    fn gate_requires_non_empty_part1() {
        assert!(!install_needs_review(&[]));
        assert!(install_needs_review(&questions()));
    }

    fn duplicate_questions() -> Vec<Question> {
        vec![
            Question::Conflict {
                incoming: s("cava-git"),
                removable: s("cava"),
            },
            Question::Conflict {
                incoming: s("cava-git"),
                removable: s("cava"),
            },
            Question::InstallIgnorepkg { name: s("glibc") },
            Question::InstallIgnorepkg { name: s("glibc") },
        ]
    }

    #[test]
    fn duplicate_questions_dedup_to_one_row_each() {
        let model = InstallReview::new(duplicate_questions());
        assert_eq!(model.questions.len(), 2);
        assert_eq!(model.conflict_checks.len(), 2);
        assert_eq!(model.ignorepkg_checks.len(), 2);
        assert_eq!(model.replace_checks.len(), 2);
        assert_eq!(model.removepkgs_checks.len(), 2);
        assert_eq!(model.holdpkgs_checks.len(), 2);
        assert!(model.conflict_checks[0]);
        assert!(!model.ignorepkg_checks[1]);
    }

    #[test]
    fn toggling_deduped_row_seals_single_consistent_answer() {
        let mut model = InstallReview::new(duplicate_questions());
        model.update(ReviewMessage::ToggleConflict(0));
        let sealed = model.seal().expect("seal succeeds");
        assert_eq!(sealed.answers.len(), 2);
        let conflict_rows = sealed
            .answers
            .iter()
            .filter(|(key, _)| matches!(key, QuestionKey::Conflict { .. }))
            .count();
        assert_eq!(conflict_rows, 1);
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Conflict { .. },
                Answer::Conflict { remove: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::InstallIgnorepkg { .. },
                Answer::InstallIgnorepkg { install: false, .. }
            )
        )));
    }

    #[test]
    fn group_members_default_to_all_selected() {
        let members = vec![s("vim"), s("git"), s("htop")];
        let model = InstallReview::new(vec![Question::GroupMembers {
            group: s("tools"),
            members: members.clone(),
        }]);
        let sealed = model.seal().expect("seal succeeds");
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::GroupMembers { .. },
                Answer::GroupMembers { selected }
            ) if selected == &members
        )));
    }

    fn empty_drift() -> pakajo::question::revalidate::ReviewDrift {
        pakajo::question::revalidate::ReviewDrift {
            added: BTreeSet::new(),
            changed: BTreeSet::new(),
            removed: BTreeSet::new(),
            summary: pakajo::question::review::ReviewDelta::default(),
        }
    }

    fn drift_with(
        added: Vec<QuestionKey>,
        changed: Vec<QuestionKey>,
    ) -> pakajo::question::revalidate::ReviewDrift {
        pakajo::question::revalidate::ReviewDrift {
            added: added.into_iter().collect(),
            changed: changed.into_iter().collect(),
            removed: BTreeSet::new(),
            summary: pakajo::question::review::ReviewDelta::default(),
        }
    }

    #[test]
    fn refresh_preserves_toggled_answers_on_unchanged_questions() {
        let mut model = InstallReview::new(questions());
        model.update(ReviewMessage::ToggleConflict(0));
        model.update(ReviewMessage::SelectProvider {
            depend: s("virt"),
            idx: 1,
        });
        let before = model.answers();
        model.refresh(questions(), &empty_drift());
        assert_eq!(model.answers(), before);
        assert!(!model.conflict_checks[0]);
        assert_eq!(model.provider_choices.get("virt"), Some(&1));
        assert!(model.highlighted.is_empty());
        assert!(!model.approving);
    }

    #[test]
    fn refresh_defaults_added_questions_and_highlights() {
        let mut model = InstallReview::new(questions());
        model.update(ReviewMessage::SelectProvider {
            depend: s("virt"),
            idx: 1,
        });
        let drifted_provider = Question::SelectProvider {
            depend: s("virt"),
            candidates: vec![candidate("qemu"), candidate("kvm"), candidate("virtualbox")],
        };
        let added_ignorepkg = Question::InstallIgnorepkg { name: s("yay") };
        let fresh = vec![
            questions().into_iter().next().expect("conflict first"),
            drifted_provider.clone(),
            added_ignorepkg.clone(),
        ];
        let drift = drift_with(vec![added_ignorepkg.key()], vec![drifted_provider.key()]);
        model.refresh(fresh, &drift);
        assert!(model.conflict_checks[0]);
        assert_eq!(model.provider_choices.get("virt"), Some(&0));
        assert!(!model.ignorepkg_checks[2]);
        assert_eq!(
            model.highlighted,
            BTreeSet::from([added_ignorepkg.key(), drifted_provider.key()])
        );
        assert_eq!(model.added, BTreeSet::from([added_ignorepkg.key()]));
    }

    #[test]
    fn refresh_clears_stale_highlight_on_stable_refresh() {
        let mut model = InstallReview::new(questions());
        let added_ignorepkg = Question::InstallIgnorepkg { name: s("yay") };
        let mut fresh = questions();
        fresh.push(added_ignorepkg.clone());
        model.refresh(fresh, &drift_with(vec![added_ignorepkg.key()], Vec::new()));
        assert!(!model.highlighted.is_empty());
        let stable = model.questions.clone();
        model.refresh(stable, &empty_drift());
        assert!(model.highlighted.is_empty());
        assert!(model.added.is_empty());
    }

    fn holdpkgs_questions() -> Vec<Question> {
        vec![
            Question::HoldPkgs {
                names: vec![s("linux"), s("linux-headers")],
            },
            Question::RemovePkgs {
                names: vec![s("ghost-pkg")],
                kind: TransactionKind::Remove,
            },
            Question::RemovePkgs {
                names: vec![s("broken-dep")],
                kind: TransactionKind::Install,
            },
        ]
    }

    #[test]
    fn toggle_holdpkgs_seals_proceed_for_toggled_only() {
        let mut model = InstallReview::new(vec![
            Question::HoldPkgs {
                names: vec![s("linux")],
            },
            Question::HoldPkgs {
                names: vec![s("nvidia")],
            },
        ]);
        model.update(ReviewMessage::ToggleHoldpkgs(0));
        assert!(model.holdpkgs_checks[0]);
        assert!(!model.holdpkgs_checks[1]);
        let sealed = model.seal().expect("seal succeeds");
        let proceeded: Vec<(Vec<String>, bool)> = sealed
            .answers
            .iter()
            .filter_map(|(_, answer)| match answer {
                Answer::HoldPkgs { names, proceed } => Some((names.clone(), *proceed)),
                _ => None,
            })
            .collect();
        assert_eq!(
            proceeded,
            vec![(vec![s("linux")], true), (vec![s("nvidia")], false)]
        );
    }

    #[test]
    fn refresh_preserves_holdpkgs_toggles_on_identical_questions() {
        let mut model = InstallReview::new(holdpkgs_questions());
        model.update(ReviewMessage::ToggleHoldpkgs(0));
        let before = model.answers();
        model.refresh(holdpkgs_questions(), &empty_drift());
        assert_eq!(model.answers(), before);
        assert!(model.holdpkgs_checks[0]);
        assert!(model.highlighted.is_empty());
    }

    #[test]
    fn refresh_changed_holdpkgs_names_reset_and_highlight() {
        let mut model = InstallReview::new(holdpkgs_questions());
        model.update(ReviewMessage::ToggleHoldpkgs(0));
        let changed = Question::HoldPkgs {
            names: vec![s("linux-lts")],
        };
        let mut fresh = holdpkgs_questions();
        fresh[0] = changed.clone();
        model.refresh(fresh, &drift_with(Vec::new(), vec![changed.key()]));
        assert!(!model.holdpkgs_checks[0]);
        assert!(model.highlighted.contains(&changed.key()));
    }

    #[test]
    fn can_confirm_requires_gated_checks() {
        let mut model = InstallReview::new(holdpkgs_questions());
        assert!(!model.can_confirm());
        model.update(ReviewMessage::ToggleHoldpkgs(0));
        assert!(model.can_confirm());
        model.update(ReviewMessage::ToggleRemovepkgs(1));
        assert!(!model.can_confirm());
        model.update(ReviewMessage::ToggleRemovepkgs(1));
        assert!(model.can_confirm());
    }

    #[test]
    fn can_confirm_true_without_gated_questions() {
        let model = InstallReview::new(vec![Question::InstallIgnorepkg { name: s("glibc") }]);
        assert!(model.can_confirm());
    }

    #[test]
    fn seal_after_refresh_reflects_preserved_answers() {
        let mut model = InstallReview::new(questions());
        model.update(ReviewMessage::ToggleConflict(0));
        model.update(ReviewMessage::ToggleReplace(2));
        model.update(ReviewMessage::ToggleIgnorepkg(3));
        model.update(ReviewMessage::ToggleRemovepkgs(4));
        model.update(ReviewMessage::SelectProvider {
            depend: s("virt"),
            idx: 1,
        });
        model.refresh(questions(), &empty_drift());
        let sealed = model.seal().expect("seal succeeds");
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Conflict { .. },
                Answer::Conflict { remove: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::Replace { .. },
                Answer::Replace { replace: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::InstallIgnorepkg { .. },
                Answer::InstallIgnorepkg { install: true, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::RemovePkgs { .. },
                Answer::RemovePkgs { skip: false, .. }
            )
        )));
        assert!(sealed.answers.iter().any(|(key, answer)| matches!(
            (key, answer),
            (
                QuestionKey::SelectProvider { .. },
                Answer::SelectProvider { name, .. }
            ) if name == "virtualbox"
        )));
    }
}
