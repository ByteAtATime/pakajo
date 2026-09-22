use std::collections::HashMap;

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, button, checkbox, container, dialog, radio, scrollable, text};

use pakajo::progress::InstallKind;
use pakajo::question::approvals::{SealedApprovals, seal as seal_answers};
use pakajo::question::model::{Answer, Question, QuestionKey};
use pakajo::question::{
    Approvals, Conflict, ProviderApproval, ProviderCandidate, QuestionSet, encode_approvals,
};

use super::TransactionMessage;
use crate::Element;

#[derive(Clone, Debug)]
pub enum ReviewMessage {
    ToggleConflict(usize),
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
    pub(super) approving: bool,
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
            approving: false,
        }
    }

    pub(crate) fn update(&mut self, message: ReviewMessage) {
        match message {
            ReviewMessage::ToggleConflict(i) => {
                if let Some(check) = self.conflict_checks.get_mut(i) {
                    *check = !*check;
                }
            }
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

    pub(crate) fn view(&self, name: &str, kind: InstallKind) -> Element<'_> {
        let body =
            review_body(&self.qs, &self.conflict_checks, &self.provider_choices).map(|selection| {
                crate::Message::Transaction(TransactionMessage::Review(match selection {
                    ReviewSelection::ToggleConflict(i) => ReviewMessage::ToggleConflict(i),
                    ReviewSelection::SelectProvider { depend, idx } => {
                        ReviewMessage::SelectProvider { depend, idx }
                    }
                }))
            });

        let is_protected_removal = matches!(kind, InstallKind::Remove) && !self.qs.held.is_empty();
        let approve = crate::Message::Transaction(TransactionMessage::ApproveReview);
        let confirm: Element<'_> = if self.approving {
            button::suggested("Loading...").into()
        } else if is_protected_removal {
            button::destructive("Remove anyway")
                .on_press(approve)
                .into()
        } else {
            let label = match kind {
                InstallKind::Remove => "Confirm removal",
                InstallKind::Install | InstallKind::Upgrade => "Confirm",
            };
            button::suggested(label).on_press(approve).into()
        };
        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelReview,
        ));

        let title = match kind {
            InstallKind::Remove => format!("Review removal of {name}"),
            InstallKind::Install | InstallKind::Upgrade => {
                format!("Review installation of {name}")
            }
        };
        dialog()
            .title(title)
            .control(scrollable(body).height(Length::Fixed(400.0)))
            .primary_action(confirm)
            .secondary_action(cancel)
            .into()
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
    pub(crate) provider_choices: HashMap<String, usize>,
    pub(super) approving: bool,
}

impl InstallReview {
    pub(crate) fn new(questions: Vec<Question>) -> Self {
        Self {
            conflict_checks: questions
                .iter()
                .map(|question| matches!(question, Question::Conflict { .. }))
                .collect(),
            provider_choices: questions
                .iter()
                .filter_map(|question| match question {
                    Question::SelectProvider { depend, .. } => Some((depend.clone(), 0)),
                    _ => None,
                })
                .collect(),
            questions,
            approving: false,
        }
    }

    pub(crate) fn update(&mut self, message: ReviewMessage) {
        match message {
            ReviewMessage::ToggleConflict(i) => {
                if let Some(check) = self.conflict_checks.get_mut(i) {
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
                remove: self.conflict_checks.get(i).copied().unwrap_or(false),
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
                replace: true,
            },
            Question::InstallIgnorepkg { name } => Answer::InstallIgnorepkg {
                name: name.clone(),
                install: false,
            },
            Question::RemovePkgs { names } => Answer::RemovePkgs {
                names: names.clone(),
                skip: false,
            },
            Question::GroupMembers { .. } => Answer::GroupMembers { selected: vec![] },
            Question::Proceed(_) => Answer::Proceed,
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

    pub(crate) fn seal(&self) -> anyhow::Result<SealedApprovals> {
        let answers = self
            .questions
            .iter()
            .enumerate()
            .map(|(i, question)| self.answer_for(i, question))
            .collect::<Vec<_>>();
        seal_answers(&self.questions, &answers, true)
    }

    pub(crate) fn view(&self, name: &str) -> Element<'_> {
        let body = part1_body(
            &self.questions,
            &self.conflict_checks,
            &self.provider_choices,
        )
        .map(|message| crate::Message::Transaction(TransactionMessage::Review(message)));
        let approve = crate::Message::Transaction(TransactionMessage::ApproveReview);
        let confirm: Element<'_> = if self.approving {
            button::suggested("Loading...").into()
        } else {
            button::suggested("Confirm").on_press(approve).into()
        };
        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelReview,
        ));
        dialog()
            .title(format!("Review installation of {name}"))
            .control(scrollable(body).height(Length::Fixed(400.0)))
            .primary_action(confirm)
            .secondary_action(cancel)
            .into()
    }
}

pub(crate) fn install_needs_review(part1: &[Question]) -> bool {
    !part1.is_empty()
}

pub(crate) fn bridge_approvals(sealed: &SealedApprovals) -> anyhow::Result<String> {
    let mut approved_conflicts = Vec::new();
    let mut approved_providers = Vec::new();
    for (key, answer) in &sealed.answers {
        match (key, answer) {
            (
                QuestionKey::Conflict { .. },
                Answer::Conflict {
                    incoming,
                    removable,
                    remove: true,
                },
            ) => approved_conflicts.push(Conflict {
                incoming: incoming.clone(),
                removable: removable.clone(),
            }),
            (QuestionKey::SelectProvider { depend }, Answer::SelectProvider { name, repo }) => {
                approved_providers.push(ProviderApproval {
                    depend: depend.clone(),
                    provider_name: name.clone(),
                    provider_repo: repo.clone(),
                })
            }
            _ => {}
        }
    }
    encode_approvals(&Approvals {
        approved_conflicts,
        approved_providers,
        ..Default::default()
    })
}

fn info_caption(question: &Question) -> Option<String> {
    match question {
        Question::Replace { old, new, .. } => Some(format!("{old} will be replaced by {new}")),
        Question::InstallIgnorepkg { name } => {
            Some(format!("{name} is in IgnorePkg and will be skipped"))
        }
        Question::RemovePkgs { names } => Some(format!(
            "Unresolvable packages will abort the transaction: {}",
            names.join(", ")
        )),
        Question::GroupMembers { group, members } => Some(format!(
            "Group {group} was pre-expanded into its members: {}",
            members.join(", ")
        )),
        _ => None,
    }
}

fn part1_body<'a>(
    questions: &'a [Question],
    checks: &'a [bool],
    choices: &'a HashMap<String, usize>,
) -> cosmic::Element<'a, ReviewMessage> {
    let mut body = Column::new().spacing(16);
    let mut in_conflicts = false;
    let mut in_providers = false;
    for (i, question) in questions.iter().enumerate() {
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
                body = body.push(
                    checkbox(checked)
                        .label(label)
                        .on_toggle(move |_| ReviewMessage::ToggleConflict(i)),
                );
            }
            Question::SelectProvider { depend, candidates } => {
                if !in_providers {
                    body = body.push(text("Providers"));
                    in_providers = true;
                }
                body = body.push(text(depend.clone()));
                let selected = choices.get(depend).copied().unwrap_or(0);
                for (idx, candidate) in candidates.iter().enumerate() {
                    let label = candidate_label(candidate);
                    let depend = depend.clone();
                    body = body.push(radio(
                        text(label),
                        idx,
                        Some(selected),
                        move |chosen: usize| ReviewMessage::SelectProvider {
                            depend: depend.clone(),
                            idx: chosen,
                        },
                    ));
                }
            }
            other => {
                if let Some(caption) = info_caption(other) {
                    body = body.push(text(caption));
                }
            }
        }
    }
    body.into()
}

#[cfg(test)]
mod install_review_tests {
    use super::*;
    use pakajo::question::model::{Answer, ProviderCandidate};

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
                    assert!(!*skip)
                }
                other => panic!("unexpected sealed entry: {other:?}"),
            }
        }
    }

    #[test]
    fn bridge_emits_exact_legacy_fields_and_omits_unchecked() {
        let sealed = model_with(true, 1).seal().expect("seal succeeds");
        let payload = bridge_approvals(&sealed).expect("bridge succeeds");
        let approvals: Approvals = serde_json::from_str(&payload).expect("decodes");
        assert_eq!(
            approvals.approved_conflicts,
            vec![Conflict {
                incoming: s("cava-git"),
                removable: s("cava"),
            }]
        );
        assert_eq!(
            approvals.approved_providers,
            vec![ProviderApproval {
                depend: s("virt"),
                provider_name: s("virtualbox"),
                provider_repo: Some(s("extra")),
            }]
        );
        assert!(approvals.approved_held.is_empty());
        assert!(approvals.approved_groups.is_empty());

        let unbound = model_with(false, 0).seal().expect("seal succeeds");
        let approvals: Approvals =
            serde_json::from_str(&bridge_approvals(&unbound).expect("bridge succeeds"))
                .expect("decodes");
        assert!(approvals.approved_conflicts.is_empty());
        assert_eq!(
            approvals.approved_providers[0].provider_name, "qemu",
            "first candidate is the presented default"
        );
    }

    #[test]
    fn gate_requires_non_empty_part1() {
        assert!(!install_needs_review(&[]));
        assert!(install_needs_review(&questions()));
    }
}
