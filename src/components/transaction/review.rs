use std::collections::HashMap;

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, button, checkbox, container, dialog, radio, scrollable, text};

use pakajo::progress::InstallKind;
use pakajo::question::{ProviderCandidate, QuestionSet};

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
