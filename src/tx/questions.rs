use std::cell::RefCell;
use std::rc::Rc;

use crate::question::model::{Answer, ProviderCandidate, Question, QuestionKey};
use crate::question::source::{AnswerSource, FailClosed, SourceDecision};

pub struct QuestionSession {
    source: Box<dyn AnswerSource>,
    denied: Option<FailClosed>,
    recorded: Vec<(Question, Answer)>,
}

impl QuestionSession {
    fn new(source: Box<dyn AnswerSource>) -> Self {
        Self {
            source,
            denied: None,
            recorded: Vec::new(),
        }
    }

    pub fn attach(handle: &alpm::Alpm, source: Box<dyn AnswerSource>) -> Rc<RefCell<Self>> {
        let session = Rc::new(RefCell::new(Self::new(source)));
        handle.set_question_cb(session.clone(), |question, state| {
            state.borrow_mut().handle(question);
        });
        session
    }

    pub fn denied(&self) -> Option<&FailClosed> {
        self.denied.as_ref()
    }

    pub(crate) fn take_denied(&mut self) -> Option<FailClosed> {
        self.denied.take()
    }

    pub(crate) fn recorded(&self) -> Vec<(Question, Answer)> {
        self.recorded.clone()
    }

    pub(crate) fn ask_direct(&mut self, question: &Question) -> anyhow::Result<Option<Answer>> {
        match self.poll(question) {
            Ok(answer) => Ok(answer),
            Err(report) => anyhow::bail!(report),
        }
    }

    pub(crate) fn deny(&mut self, key: QuestionKey, reason: impl Into<String>) {
        self.denied = Some(FailClosed {
            key,
            reason: reason.into(),
        });
    }

    fn handle(&mut self, question: alpm::AnyQuestion) {
        if self.denied.is_some() {
            return;
        }
        match question.question() {
            alpm::Question::InstallIgnorepkg(mut asked) => {
                let name = asked.pkg().name().to_string();
                self.decide_bool(Question::InstallIgnorepkg { name }, |install| {
                    asked.set_install(install);
                });
            }
            alpm::Question::Replace(asked) => {
                let old = asked.oldpkg().name().to_string();
                let new = asked.newpkg().name().to_string();
                let repo = asked.newdb().name().to_string();
                self.decide_bool(
                    Question::Replace {
                        old,
                        new,
                        repo: Some(repo),
                    },
                    |replace| asked.set_replace(replace),
                );
            }
            alpm::Question::Conflict(mut asked) => {
                let disputed = asked.conflict();
                let incoming = disputed.package1().name().to_string();
                let removable = disputed.package2().name().to_string();
                self.decide_bool(
                    Question::Conflict {
                        incoming,
                        removable,
                    },
                    |remove| asked.set_remove(remove),
                );
            }
            alpm::Question::Corrupted(mut asked) => {
                let path = asked.filepath().to_string();
                self.decide_bool(Question::Corrupted { path }, |remove| {
                    asked.set_remove(remove);
                });
            }
            alpm::Question::RemovePkgs(mut asked) => {
                let names = asked
                    .packages()
                    .iter()
                    .map(|pkg| pkg.name().to_string())
                    .collect();
                self.decide_bool(Question::RemovePkgs { names }, |skip| {
                    asked.set_skip(skip);
                });
            }
            alpm::Question::SelectProvider(mut asked) => {
                self.answer_provider(&mut asked);
            }
            alpm::Question::ImportKey(mut asked) => {
                let fingerprint = asked.fingerprint().to_string();
                let uid = asked.uid().to_string();
                self.decide_bool(Question::ImportKey { fingerprint, uid }, |import| {
                    asked.set_import(import)
                });
            }
        }
    }

    fn decide_bool(&mut self, question: Question, apply: impl FnOnce(bool)) {
        let Some(answer) = self.ask(&question) else {
            return;
        };
        match match_bool(&question, &answer) {
            Some(value) => apply(value),
            None => self.deny(question.key(), "answer did not match question"),
        }
    }

    fn answer_provider(&mut self, asked: &mut alpm::SelectProviderQuestion<'_>) {
        let candidates = provider_candidates(asked);
        let question = Question::SelectProvider {
            depend: asked.depend().to_string(),
            candidates: candidates.clone(),
        };
        let Some(answer) = self.ask(&question) else {
            return;
        };
        let Answer::SelectProvider { name, repo } = answer else {
            self.deny(question.key(), "answer did not match question");
            return;
        };
        match resolve_provider_index(&candidates, &name, repo.as_deref()) {
            Some(index) => asked.set_index(index as i32),
            None => self.deny(question.key(), format!("provider {name} is not offered")),
        }
    }

    fn ask(&mut self, question: &Question) -> Option<Answer> {
        self.poll(question).unwrap_or(None)
    }

    fn poll(&mut self, question: &Question) -> Result<Option<Answer>, String> {
        if self.denied.is_some() {
            return Ok(None);
        }
        match self.source.answer(question) {
            SourceDecision::Answer(Answer::Stop) => Ok(None),
            SourceDecision::Answer(answer) => {
                self.recorded.push((question.clone(), answer.clone()));
                Ok(Some(answer))
            }
            SourceDecision::Abort(denied) => {
                let report = denied.to_string();
                self.denied = Some(denied);
                Err(report)
            }
        }
    }
}

fn match_bool(question: &Question, answer: &Answer) -> Option<bool> {
    match (question, answer) {
        (Question::InstallIgnorepkg { .. }, Answer::InstallIgnorepkg { install, .. }) => {
            Some(*install)
        }
        (Question::Replace { .. }, Answer::Replace { replace, .. }) => Some(*replace),
        (Question::Conflict { .. }, Answer::Conflict { remove, .. }) => Some(*remove),
        (Question::Corrupted { .. }, Answer::Corrupted { remove, .. }) => Some(*remove),
        (Question::RemovePkgs { .. }, Answer::RemovePkgs { skip, .. }) => Some(*skip),
        (Question::ImportKey { .. }, Answer::ImportKey { import, .. }) => Some(*import),
        _ => None,
    }
}

fn resolve_provider_index(
    candidates: &[ProviderCandidate],
    name: &str,
    repo: Option<&str>,
) -> Option<usize> {
    let mut matches = candidates.iter().enumerate().filter(|(_, candidate)| {
        candidate.name == name
            && match (&candidate.repo, repo) {
                (Some(have), Some(want)) => have == want,
                (None, Some(_)) => false,
                _ => true,
            }
    });
    let (index, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(index)
}

fn provider_candidates(asked: &alpm::SelectProviderQuestion<'_>) -> Vec<ProviderCandidate> {
    asked
        .providers()
        .into_iter()
        .map(|pkg| ProviderCandidate {
            name: pkg.name().to_string(),
            repo: pkg.db().map(|db| db.name().to_string()),
            version: Some(pkg.version().to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, repo: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: name.into(),
            repo: Some(repo.into()),
            version: None,
        }
    }

    #[test]
    fn session_records_answers_but_skips_stops() {
        use crate::question::source::ExploreDefaults;
        let mut session = QuestionSession::new(Box::new(ExploreDefaults));
        let group = Question::GroupMembers {
            group: "tools".to_string(),
            members: vec!["a".to_string(), "b".to_string()],
        };
        let answered = session.ask_direct(&group).unwrap();
        assert_eq!(
            answered,
            Some(Answer::GroupMembers {
                selected: vec!["a".to_string(), "b".to_string()]
            })
        );
        let proceed = Question::Proceed(crate::events::TransactionSummary::default());
        assert_eq!(session.ask_direct(&proceed).unwrap(), None);
        assert_eq!(session.recorded(), vec![(group, answered.unwrap())]);
    }

    #[test]
    fn provider_index_resolves_by_name_and_rejects_ambiguity() {
        let straight = [candidate("foo", "repo1"), candidate("bar", "repo2")];
        let swapped = [candidate("bar", "repo2"), candidate("foo", "repo1")];
        assert_eq!(resolve_provider_index(&straight, "bar", None), Some(1));
        assert_eq!(resolve_provider_index(&swapped, "bar", None), Some(0));
        assert_eq!(resolve_provider_index(&straight, "ghost", None), None);
        assert_eq!(
            resolve_provider_index(&straight, "bar", Some("repo1")),
            None
        );
        let duplicated = [candidate("foo", "repo1"), candidate("foo", "repo1")];
        assert_eq!(resolve_provider_index(&duplicated, "foo", None), None);
    }
}
