use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cosmic::iced::alignment::Vertical;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::icon;
use cosmic::widget::{
    Column, Row, button, checkbox, container, dialog, radio, scrollable, space, text,
};

use crate::components::icons::{git_merge, shuffle, triangle_alert};
use crate::components::theme::{card_style, destructive_color, muted_color, warning_color};

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
    ApproveHolds,
    ToggleConsentHolds,
    Noop,
}

fn status_pill(caption: &str) -> cosmic::Element<'static, ReviewMessage> {
    let colored = |theme: &cosmic::Theme| Color::from(theme.cosmic().warning.base);
    container(text(caption.to_uppercase()))
        .padding([2.0, 8.0])
        .style(move |theme: &cosmic::Theme| {
            let colored = colored(theme);
            container::Style {
                text_color: Some(colored),
                background: Some(Background::Color(Color { a: 0.10, ..colored })),
                border: Border {
                    radius: theme.cosmic().corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn tinted_elem<'a>(
    content: impl Into<cosmic::Element<'a, ReviewMessage>>,
    tint: fn(&cosmic::Theme) -> Color,
) -> cosmic::Element<'a, ReviewMessage> {
    container(content.into())
        .style(move |theme: &cosmic::Theme| container::Style {
            text_color: Some(tint(theme)),
            ..Default::default()
        })
        .into()
}

fn muted_elem<'a>(
    content: impl Into<cosmic::Element<'a, ReviewMessage>>,
) -> cosmic::Element<'a, ReviewMessage> {
    tinted_elem(content, muted_color)
}

fn tinted_icon(
    handle: icon::Handle,
    tint: fn(&cosmic::Theme) -> Color,
) -> cosmic::Element<'static, ReviewMessage> {
    tinted_elem(icon(handle).size(16), tint)
}

fn question_card<'a>(
    line: Vec<cosmic::Element<'a, ReviewMessage>>,
    caption: Option<&str>,
    extra: Vec<cosmic::Element<'a, ReviewMessage>>,
) -> cosmic::Element<'a, ReviewMessage> {
    let mut line = Row::with_children(line).spacing(10);
    line = line.push(space::horizontal());
    if let Some(caption) = caption {
        line = line.push(status_pill(caption));
    }
    let mut card = Column::new()
        .spacing(6)
        .push(line.align_y(Vertical::Center));
    for element in extra {
        card = card.push(element);
    }
    container(card)
        .padding([10.0, 14.0])
        .width(Length::Fill)
        .style(card_style)
        .into()
}

fn card_button_style(theme: &cosmic::Theme) -> button::Style {
    let cosmic = theme.cosmic();
    button::Style {
        text_color: Some(Color::from(cosmic.background(false).on)),
        background: Some(Background::Color(Color::from(
            cosmic.background(false).component.base,
        ))),
        border_radius: cosmic.corner_radii.radius_s.into(),
        border_width: 1.0,
        border_color: Color::from(cosmic.background(false).divider),
        ..Default::default()
    }
}

fn card_button_hover_style(theme: &cosmic::Theme) -> button::Style {
    let mut style = card_button_style(theme);
    let mut background = Color::from(theme.cosmic().background(false).component.base);
    background = Color {
        a: (background.a * 1.6).min(1.0),
        ..background
    };
    style.background = Some(Background::Color(background));
    style
}

fn clickable_card<'a>(
    content: cosmic::Element<'a, ReviewMessage>,
    message: ReviewMessage,
) -> cosmic::Element<'a, ReviewMessage> {
    button::custom(content)
        .on_press(message)
        .padding([10.0, 14.0])
        .width(Length::Fill)
        .class(cosmic::theme::Button::Custom {
            active: Box::new(|_, theme| card_button_style(theme)),
            disabled: Box::new(card_button_style),
            hovered: Box::new(|_, theme| card_button_hover_style(theme)),
            pressed: Box::new(|_, theme| card_button_hover_style(theme)),
        })
        .into()
}

fn toggle_card<'a>(
    label: cosmic::Element<'a, ReviewMessage>,
    detail: Option<String>,
    checked: bool,
    message: ReviewMessage,
    caption: Option<&str>,
) -> cosmic::Element<'a, ReviewMessage> {
    let mut text_column = Column::new().push(label);
    if let Some(detail) = detail {
        text_column = text_column.push(muted_elem(text::monotext(detail)));
    }
    let mut line = Row::new()
        .align_y(Vertical::Center)
        .spacing(10)
        .push(checkbox(checked).on_toggle(|_| ReviewMessage::Noop))
        .push(text_column)
        .push(space::horizontal());
    if let Some(caption) = caption {
        line = line.push(status_pill(caption));
    }
    clickable_card(line.into(), message)
}

fn provider_candidate_row<'a>(
    candidate: &pakajo::question::model::ProviderCandidate,
) -> cosmic::Element<'a, ReviewMessage> {
    let qualified = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    let mut row = Row::new()
        .align_y(Vertical::Center)
        .spacing(8)
        .push(text::monotext(qualified));
    if let Some(version) = &candidate.version {
        row = row.push(muted_elem(text(version.clone())));
    }
    row.into()
}

fn section<'a>(
    lead: cosmic::Element<'static, ReviewMessage>,
    title: &str,
    items: Vec<cosmic::Element<'a, ReviewMessage>>,
) -> cosmic::Element<'a, ReviewMessage> {
    let count = items.len();
    let label = if count == 1 {
        title.to_string()
    } else {
        format!("{title} · {count}")
    };
    Column::new()
        .spacing(8)
        .push(
            Row::new()
                .align_y(Vertical::Center)
                .spacing(8)
                .push(lead)
                .push(text(label)),
        )
        .push(Column::with_children(items).spacing(8))
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
    holds_confirmed: bool,
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
            holds_confirmed: false,
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
        self.holds_confirmed = false;
    }

    pub(crate) fn update(&mut self, message: ReviewMessage) {
        match message {
            ReviewMessage::ToggleConflict(i) => flip(&mut self.conflict_checks, i),
            ReviewMessage::ToggleReplace(i) => flip(&mut self.replace_checks, i),
            ReviewMessage::ToggleIgnorepkg(i) => flip(&mut self.ignorepkg_checks, i),
            ReviewMessage::ToggleRemovepkgs(i) => flip(&mut self.removepkgs_checks, i),
            ReviewMessage::ToggleHoldpkgs(i) => flip(&mut self.holdpkgs_checks, i),
            ReviewMessage::Noop => {}
            ReviewMessage::SelectProvider { depend, idx } => {
                self.provider_choices.insert(depend, idx);
            }
            ReviewMessage::ApproveHolds => {
                if self.can_confirm() {
                    self.holds_confirmed = true;
                }
            }
            ReviewMessage::ToggleConsentHolds => {
                let consent = self.all_holds_checked();
                for (i, question) in self.questions.iter().enumerate() {
                    if matches!(question, Question::HoldPkgs { .. })
                        && let Some(check) = self.holdpkgs_checks.get_mut(i)
                    {
                        *check = !consent;
                    }
                }
            }
        }
    }

    fn all_holds_checked(&self) -> bool {
        self.questions.iter().enumerate().all(|(i, question)| {
            !matches!(question, Question::HoldPkgs { .. })
                || self.holdpkgs_checks.get(i).copied().unwrap_or(false)
        })
    }

    fn in_holds_stage(&self) -> bool {
        self.questions
            .iter()
            .any(|question| matches!(question, Question::HoldPkgs { .. }))
            && !self.holds_confirmed
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
        let holds_stage = self.in_holds_stage();
        let body = if holds_stage {
            holds_body(self)
        } else {
            part1_body(self, holds_stage)
        }
        .map(|message| crate::Message::Transaction(TransactionMessage::Review(message)));
        let gated = self.can_confirm();
        let (confirm, title): (Element<'_>, String) = if self.approving {
            (
                button::suggested("Loading...").into(),
                install_review_title(name, kind),
            )
        } else if holds_stage {
            let names = hold_names(self);
            let stage_title = if names.len() == 1 {
                format!("Remove held package {}", names[0])
            } else {
                "Held packages would be removed".to_string()
            };
            let action = button::destructive("Remove anyway");
            let action = if gated {
                action
                    .on_press(crate::Message::Transaction(TransactionMessage::Review(
                        ReviewMessage::ApproveHolds,
                    )))
                    .into()
            } else {
                action.into()
            };
            (action, stage_title)
        } else {
            let approve = crate::Message::Transaction(TransactionMessage::ApproveReview);
            let action = if matches!(kind, InstallKind::Remove) {
                button::destructive(install_confirm_label(kind))
            } else {
                button::suggested(install_confirm_label(kind))
            };
            let action: Element<'_> = if gated {
                action.on_press(approve).into()
            } else {
                action.into()
            };
            (action, install_review_title(name, kind))
        };
        let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
            TransactionMessage::CancelReview,
        ));
        dialog()
            .title(title)
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

fn removepkgs_label_short(kind: &TransactionKind) -> &'static str {
    match kind {
        TransactionKind::Remove => "Skip missing packages and continue",
        TransactionKind::Install => "Skip unresolvable packages and continue",
    }
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

fn hold_names(review: &InstallReview) -> Vec<String> {
    review
        .questions
        .iter()
        .filter_map(|question| match question {
            Question::HoldPkgs { names } => Some(names.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn holds_body<'a>(review: &'a InstallReview) -> cosmic::Element<'a, ReviewMessage> {
    let names = hold_names(review);
    let single = names.len() == 1;
    let lead = if single {
        "This package is protected by HoldPkg:"
    } else {
        "You are about to remove packages protected by HoldPkg:"
    };
    let warning: cosmic::Element<'a, ReviewMessage> = Row::new()
        .spacing(8)
        .push(tinted_icon(triangle_alert(), destructive_color))
        .push(
            container(text(
                "Removing them can break your system. Unless you are sure you know what you are doing, this is probably not what you want.",
            ))
            .style(|theme: &cosmic::Theme| container::Style {
                text_color: Some(destructive_color(theme)),
                ..Default::default()
            }),
        )
        .into();
    let consent_label = if single {
        "I understand, remove it anyway"
    } else {
        "I understand, remove them anyway"
    };
    Column::new()
        .spacing(14)
        .width(Length::Fill)
        .push(text::body(lead))
        .push(
            container(text::monotext(names.join("\n")))
                .width(Length::Fill)
                .padding([10.0, 14.0])
                .style(card_style),
        )
        .push(warning)
        .push(
            checkbox(review.all_holds_checked())
                .label(consent_label)
                .on_toggle(|_| ReviewMessage::ToggleConsentHolds),
        )
        .into()
}

fn part1_body<'a>(
    review: &'a InstallReview,
    holds_stage: bool,
) -> cosmic::Element<'a, ReviewMessage> {
    let InstallReview {
        questions,
        conflict_checks: checks,
        replace_checks,
        ignorepkg_checks,
        removepkgs_checks,
        provider_choices: choices,
        highlighted,
        added,
        ..
    } = review;
    let mut conflicts: Vec<cosmic::Element<'a, ReviewMessage>> = Vec::new();
    let mut providers: Vec<cosmic::Element<'a, ReviewMessage>> = Vec::new();
    let mut decisions: Vec<cosmic::Element<'a, ReviewMessage>> = Vec::new();
    for (i, question) in questions.iter().enumerate() {
        let caption = highlight_caption(&question.key(), highlighted, added);
        if matches!(question, Question::HoldPkgs { .. }) != holds_stage {
            continue;
        }
        match question {
            Question::Conflict {
                incoming,
                incoming_version,
                removable,
                removable_version,
                conflict_reason,
            } => {
                let checked = checks.get(i).copied().unwrap_or(false);
                let label: cosmic::Element<'a, ReviewMessage> = Row::new()
                    .align_y(Vertical::Center)
                    .push(muted_elem(text("Replace ".to_string())))
                    .push(text(removable.clone()))
                    .push(muted_elem(text(format!("-{removable_version} with "))))
                    .push(text(incoming.clone()))
                    .push(muted_elem(text(format!("-{incoming_version}"))))
                    .into();
                conflicts.push(toggle_card(
                    label,
                    conflict_reason.clone(),
                    checked,
                    ReviewMessage::ToggleConflict(i),
                    caption,
                ));
            }
            Question::SelectProvider { depend, candidates } => {
                let selected = choices.get(depend).copied().unwrap_or(0);
                let mut group = Column::new().spacing(8);
                for (idx, candidate) in candidates.iter().enumerate() {
                    let depend = depend.clone();
                    group = group.push(radio(
                        provider_candidate_row(candidate),
                        idx,
                        Some(selected),
                        move |chosen: usize| ReviewMessage::SelectProvider {
                            depend: depend.clone(),
                            idx: chosen,
                        },
                    ));
                }
                let line = vec![
                    muted_elem(text("Provider for")),
                    text::monotext(depend.clone()).into(),
                ];
                providers.push(question_card(line, caption, vec![group.into()]));
            }
            Question::Replace { old, new, repo } => {
                let checked = replace_checks.get(i).copied().unwrap_or(true);
                let old_name = match repo {
                    Some(repo) => format!("{repo}/{old}"),
                    None => old.clone(),
                };
                decisions.push(toggle_card(
                    text(format!("{old_name} is replaced by {new}")).into(),
                    None,
                    checked,
                    ReviewMessage::ToggleReplace(i),
                    caption,
                ));
            }
            Question::InstallIgnorepkg { name } => {
                let checked = ignorepkg_checks.get(i).copied().unwrap_or(false);
                decisions.push(toggle_card(
                    text(format!("Install {name} anyway (in IgnorePkg)")).into(),
                    None,
                    checked,
                    ReviewMessage::ToggleIgnorepkg(i),
                    caption,
                ));
            }
            Question::RemovePkgs { names, kind } => {
                let checked = removepkgs_checks.get(i).copied().unwrap_or(true);
                decisions.push(toggle_card(
                    text(removepkgs_label_short(kind)).into(),
                    Some(names.join(", ")),
                    checked,
                    ReviewMessage::ToggleRemovepkgs(i),
                    caption,
                ));
            }
            other => {
                if let Some(note) = info_caption(other) {
                    let line = vec![muted_elem(text(note))];
                    decisions.push(question_card(line, caption, Vec::new()));
                } else if let Some(flag) = caption {
                    let key = question.key();
                    let line = vec![muted_elem(text(format!("{key:?}")))];
                    decisions.push(question_card(line, Some(flag), Vec::new()));
                }
            }
        }
    }
    let mut body = Column::new().spacing(20);
    if !conflicts.is_empty() {
        body = body.push(section(
            tinted_icon(git_merge(), destructive_color),
            "Conflicts",
            conflicts,
        ));
    }
    if !providers.is_empty() {
        body = body.push(section(
            tinted_icon(shuffle(), muted_color),
            "Providers",
            providers,
        ));
    }
    if !decisions.is_empty() {
        body = body.push(section(
            tinted_icon(triangle_alert(), warning_color),
            "Decisions",
            decisions,
        ));
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
