use std::collections::HashMap;

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::{Column, Row, button, checkbox, container, radio, scrollable, space, text};
use pakajo::question::{ProviderCandidate, QuestionSet};

use super::TransactionMessage;
use crate::Element;

#[derive(Clone, Debug)]
pub enum ReviewMessage {
    ToggleConflict(usize),
    SelectProvider { depend: String, idx: usize },
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

    pub(crate) fn view(&self, name: &str) -> Element<'_> {
        let title = format!("Review installation of {}", name);
        let mut col = Column::new().spacing(16).push(text(title));

        if !self.qs.conflicts.is_empty() {
            col = col.push(text("Conflicts"));
            for (i, conflict) in self.qs.conflicts.iter().enumerate() {
                let label = format!("Replace {} with {}", conflict.removable, conflict.incoming);
                let checked = self.conflict_checks.get(i).copied().unwrap_or(false);
                let item = checkbox(checked).label(label).on_toggle(move |_| {
                    crate::Message::Transaction(TransactionMessage::Review(
                        ReviewMessage::ToggleConflict(i),
                    ))
                });
                col = col.push(item);
            }
        }

        if !self.qs.providers.is_empty() {
            col = col.push(text("Providers"));
            for prompt in &self.qs.providers {
                col = col.push(text(prompt.depend.clone()));
                let selected = self
                    .provider_choices
                    .get(&prompt.depend)
                    .copied()
                    .unwrap_or(0);
                for (idx, candidate) in prompt.candidates.iter().enumerate() {
                    let label = candidate_label(candidate);
                    let depend = prompt.depend.clone();
                    let item = radio(text(label), idx, Some(selected), move |chosen: usize| {
                        crate::Message::Transaction(TransactionMessage::Review(
                            ReviewMessage::SelectProvider {
                                depend: depend.clone(),
                                idx: chosen,
                            },
                        ))
                    });
                    col = col.push(item);
                }
            }
        }

        if self.qs.had_unsupported_question {
            col = col.push(unsupported_banner(&self.qs.unsupported_summary));
        }

        col = col.push(review_footer(self.approving));
        container(scrollable(col))
            .class(cosmic::theme::Container::Dialog(true))
            .padding([24.0, 24.0])
            .width(Length::Fixed(570.0))
            .into()
    }
}

pub(crate) fn candidate_label(candidate: &ProviderCandidate) -> String {
    let qualified = match &candidate.repo {
        Some(repo) => format!("{repo}/{}", candidate.name),
        None => candidate.name.clone(),
    };
    match &candidate.version {
        Some(version) => format!("{qualified}  {version}"),
        None => qualified,
    }
}

pub(crate) fn unsupported_banner(summary: &str) -> Element<'static> {
    container(text(summary.to_string()))
        .padding([12.0, 16.0])
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            text_color: Some(Color::from(theme.cosmic().warning.on)),
            background: Some(Background::Color(Color::from(theme.cosmic().warning.base))),
            border: Border {
                radius: 8.0.into(),
                width: 1.0,
                color: Color::from(theme.cosmic().warning.base),
            },
            ..Default::default()
        })
        .into()
}

fn review_footer(approving: bool) -> Element<'static> {
    let cancel = button::standard("Cancel").on_press(crate::Message::Transaction(
        TransactionMessage::CancelReview,
    ));
    let confirm = if approving {
        button::standard("Loading...")
    } else {
        button::standard("Confirm").on_press(crate::Message::Transaction(
            TransactionMessage::ApproveReview,
        ))
    };
    Row::new()
        .spacing(8)
        .push(space::horizontal())
        .push(cancel)
        .push(confirm)
        .into()
}
