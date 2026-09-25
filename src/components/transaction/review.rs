use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, button, checkbox, container, dialog, radio, scrollable, text};

use pakajo::progress::InstallKind;
use pakajo::question::approvals::{SealedApprovals, seal as seal_answers};
use pakajo::question::model::{Answer, Question, QuestionKey, TransactionKind};
use pakajo::question::revalidate::ReviewDrift;

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

fn candidate_label(candidate: &pakajo::question::model::ProviderCandidate) -> String {
    let qualified = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    match &candidate.version {
        Some(version) => format!("{qualified}  {version}"),
        None => qualified,
    }
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
            conflict_checks.push(restored_flag(question, restored).unwrap_or(true));
            replace_checks.push(
                restored_flag(question, restored)
                    .unwrap_or(matches!(question, Question::Replace { .. })),
            );
            ignorepkg_checks.push(restored_flag(question, restored).unwrap_or(false));
            removepkgs_checks.push(restored_flag(question, restored).unwrap_or(true));
            holdpkgs_checks.push(restored_flag(question, restored).unwrap_or(false));
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
            ReviewMessage::ToggleConflict(i) => flip(&mut self.conflict_checks, i),
            ReviewMessage::ToggleReplace(i) => flip(&mut self.replace_checks, i),
            ReviewMessage::ToggleIgnorepkg(i) => flip(&mut self.ignorepkg_checks, i),
            ReviewMessage::ToggleRemovepkgs(i) => flip(&mut self.removepkgs_checks, i),
            ReviewMessage::ToggleHoldpkgs(i) => flip(&mut self.holdpkgs_checks, i),
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
                ..
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
        let body = part1_body(self)
            .map(|message| crate::Message::Transaction(TransactionMessage::Review(message)));
        let approve = crate::Message::Transaction(TransactionMessage::ApproveReview);
        let gated = self.can_confirm();
        let confirm: Element<'_> = if self.approving {
            button::suggested("Loading...").into()
        } else {
            let action = if matches!(kind, InstallKind::Remove) {
                button::destructive(install_confirm_label(kind))
            } else {
                button::suggested(install_confirm_label(kind))
            };
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

fn flip(checks: &mut [bool], i: usize) {
    if let Some(check) = checks.get_mut(i) {
        *check = !*check;
    }
}

fn restored_flag(question: &Question, restored: Option<&Answer>) -> Option<bool> {
    match (question, restored) {
        (Question::Conflict { .. }, Some(Answer::Conflict { remove, .. })) => Some(*remove),
        (Question::Replace { .. }, Some(Answer::Replace { replace, .. })) => Some(*replace),
        (Question::InstallIgnorepkg { .. }, Some(Answer::InstallIgnorepkg { install, .. })) => {
            Some(*install)
        }
        (Question::RemovePkgs { .. }, Some(Answer::RemovePkgs { skip, .. })) => Some(*skip),
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

fn with_caption<'a>(
    row: cosmic::Element<'a, ReviewMessage>,
    caption: Option<&str>,
) -> cosmic::Element<'a, ReviewMessage> {
    match caption {
        Some(note) => highlight_wrap(row, note),
        None => row,
    }
}

fn toggle_row<'a>(
    label: String,
    checked: bool,
    message: ReviewMessage,
) -> cosmic::Element<'a, ReviewMessage> {
    checkbox(checked)
        .label(label)
        .on_toggle(move |_| message.clone())
        .into()
}

fn part1_body<'a>(review: &'a InstallReview) -> cosmic::Element<'a, ReviewMessage> {
    let InstallReview {
        questions,
        conflict_checks: checks,
        replace_checks,
        ignorepkg_checks,
        removepkgs_checks,
        holdpkgs_checks,
        provider_choices: choices,
        highlighted,
        added,
        ..
    } = review;
    let mut body = Column::new().spacing(16);
    let mut in_conflicts = false;
    let mut in_providers = false;
    for (i, question) in questions.iter().enumerate() {
        let caption = highlight_caption(&question.key(), highlighted, added);
        match question {
            Question::Conflict {
                incoming,
                removable,
                ..
            } => {
                if !in_conflicts {
                    body = body.push(text("Conflicts"));
                    in_conflicts = true;
                }
                let label = format!("Replace {} with {}", removable, incoming);
                let checked = checks.get(i).copied().unwrap_or(false);
                let row: cosmic::Element<'a, ReviewMessage> =
                    toggle_row(label, checked, ReviewMessage::ToggleConflict(i));
                body = body.push(with_caption(row, caption));
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
                body = body.push(with_caption(row, caption));
            }
            Question::Replace { old, new, .. } => {
                let label = format!("Replace {old} with {new}");
                let checked = replace_checks.get(i).copied().unwrap_or(true);
                let row: cosmic::Element<'a, ReviewMessage> =
                    toggle_row(label, checked, ReviewMessage::ToggleReplace(i));
                body = body.push(with_caption(row, caption));
            }
            Question::InstallIgnorepkg { name } => {
                let label = format!("Install {name} anyway (in IgnorePkg)");
                let checked = ignorepkg_checks.get(i).copied().unwrap_or(false);
                let row: cosmic::Element<'a, ReviewMessage> =
                    toggle_row(label, checked, ReviewMessage::ToggleIgnorepkg(i));
                body = body.push(with_caption(row, caption));
            }
            Question::RemovePkgs { names, kind } => {
                let label = removepkgs_label(names, kind);
                let checked = removepkgs_checks.get(i).copied().unwrap_or(true);
                let row: cosmic::Element<'a, ReviewMessage> =
                    toggle_row(label, checked, ReviewMessage::ToggleRemovepkgs(i));
                body = body.push(with_caption(row, caption));
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
                body = body.push(with_caption(row, caption));
            }
            other => {
                if let Some(note) = info_caption(other) {
                    let row: cosmic::Element<'a, ReviewMessage> = text(note).into();
                    body = body.push(with_caption(row, caption));
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
mod test {
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
                incoming_version: s("1.0-1"),
                removable: s("cava"),
                removable_version: s("1.0-1"),
                conflict_reason: None,
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

    fn sealed_answer<'a>(sealed: &'a SealedApprovals, key: &QuestionKey) -> &'a Answer {
        sealed
            .answers
            .iter()
            .find_map(|(k, answer)| (k == key).then_some(answer))
            .unwrap_or_else(|| panic!("missing answer for {key:?}"))
    }

    fn conflict_of(model: &InstallReview) -> QuestionKey {
        model.questions[0].key()
    }

    fn flag_of(answer: &Answer) -> Option<bool> {
        match answer {
            Answer::Conflict { remove, .. } => Some(*remove),
            Answer::Replace { replace, .. } => Some(*replace),
            Answer::InstallIgnorepkg { install, .. } => Some(*install),
            Answer::RemovePkgs { skip, .. } => Some(*skip),
            Answer::HoldPkgs { proceed, .. } => Some(*proceed),
            _ => None,
        }
    }

    fn assert_flags(sealed: &SealedApprovals, model: &InstallReview, expected: [bool; 4]) {
        for (i, key) in [0, 2, 3, 4].into_iter().enumerate() {
            assert_eq!(
                flag_of(sealed_answer(sealed, &model.questions[key].key())),
                Some(expected[i]),
                "question index {key}"
            );
        }
    }

    #[test]
    fn seal_binds_choices_and_displayed_defaults() {
        let model = model_with(true, 1);
        let sealed = model.seal().expect("seal succeeds");
        assert!(sealed.proceed);
        assert_eq!(sealed.answers.len(), 5);
        assert!(pakajo::dispatch::seal::encode_seal(&sealed).is_ok());
        assert_flags(&sealed, &model, [true, true, false, true]);
        let provider = sealed
            .answers
            .iter()
            .find_map(|(_, answer)| match answer {
                Answer::SelectProvider { name, repo } => Some((name.as_str(), repo.as_deref())),
                _ => None,
            })
            .expect("provider answer");
        assert_eq!(provider, ("virtualbox", Some("extra")));
    }

    #[test]
    fn defaults_and_toggles_seal_per_kind() {
        let mut model = InstallReview::new(questions());
        assert!(model.conflict_checks.iter().all(|checked| *checked));

        let sealed = model.seal().expect("defaults seal");
        assert_flags(&sealed, &model, [true, true, false, true]);

        model.update(ReviewMessage::ToggleConflict(0));
        model.update(ReviewMessage::ToggleReplace(2));
        model.update(ReviewMessage::ToggleIgnorepkg(3));
        model.update(ReviewMessage::ToggleRemovepkgs(4));
        let sealed = model.seal().expect("toggled seal");
        assert_flags(&sealed, &model, [false, false, true, false]);
    }

    fn duplicate_questions() -> Vec<Question> {
        let conflict = Question::Conflict {
            incoming: s("cava-git"),
            incoming_version: s("1.0-1"),
            removable: s("cava"),
            removable_version: s("1.0-1"),
            conflict_reason: None,
        };
        vec![
            conflict.clone(),
            conflict,
            Question::InstallIgnorepkg { name: s("glibc") },
            Question::InstallIgnorepkg { name: s("glibc") },
        ]
    }

    #[test]
    fn duplicate_questions_dedup_and_seal_one_answer_per_key() {
        let mut model = InstallReview::new(duplicate_questions());
        assert_eq!(model.questions.len(), 2);
        for checks in [
            &model.conflict_checks,
            &model.ignorepkg_checks,
            &model.replace_checks,
            &model.removepkgs_checks,
            &model.holdpkgs_checks,
        ] {
            assert_eq!(checks.len(), 2);
        }
        assert!(model.conflict_checks[0]);
        assert!(!model.ignorepkg_checks[1]);

        model.update(ReviewMessage::ToggleConflict(0));
        let sealed = model.seal().expect("seal succeeds");
        assert_eq!(sealed.answers.len(), 2);
        assert_eq!(
            flag_of(sealed_answer(&sealed, &conflict_of(&model))),
            Some(false)
        );
        assert_eq!(
            flag_of(sealed_answer(&sealed, &model.questions[1].key())),
            Some(false)
        );
    }

    #[test]
    fn group_members_default_to_all_selected() {
        let members = vec![s("vim"), s("git"), s("htop")];
        let model = InstallReview::new(vec![Question::GroupMembers {
            group: s("tools"),
            members: members.clone(),
        }]);
        let sealed = model.seal().expect("seal succeeds");
        assert!(matches!(
            sealed_answer(&sealed, &model.questions[0].key()),
            Answer::GroupMembers { selected } if selected == &members
        ));
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
    fn can_confirm_tracks_gated_checks() {
        let mut model = InstallReview::new(holdpkgs_questions());
        assert!(!model.can_confirm());
        model.update(ReviewMessage::ToggleHoldpkgs(0));
        assert!(model.can_confirm());
        model.update(ReviewMessage::ToggleRemovepkgs(1));
        assert!(!model.can_confirm());
        model.update(ReviewMessage::ToggleRemovepkgs(1));
        assert!(model.can_confirm());

        let ungated = InstallReview::new(vec![Question::InstallIgnorepkg { name: s("glibc") }]);
        assert!(ungated.can_confirm());
    }
}
