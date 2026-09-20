use serde::{Deserialize, Serialize};

use crate::events::TransactionSummary;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderCandidate {
    pub name: String,
    pub repo: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Question {
    Conflict {
        incoming: String,
        removable: String,
    },
    SelectProvider {
        depend: String,
        candidates: Vec<ProviderCandidate>,
    },
    Replace {
        old: String,
        new: String,
        repo: Option<String>,
    },
    InstallIgnorepkg {
        name: String,
    },
    RemovePkgs {
        names: Vec<String>,
    },
    Corrupted {
        path: String,
    },
    ImportKey {
        fingerprint: String,
        uid: String,
    },
    Proceed(TransactionSummary),
    GroupMembers {
        group: String,
        members: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Answer {
    Conflict {
        incoming: String,
        removable: String,
        remove: bool,
    },
    SelectProvider {
        name: String,
        repo: Option<String>,
    },
    Replace {
        old: String,
        new: String,
        replace: bool,
    },
    InstallIgnorepkg {
        name: String,
        install: bool,
    },
    RemovePkgs {
        names: Vec<String>,
        skip: bool,
    },
    Corrupted {
        path: String,
        remove: bool,
    },
    ImportKey {
        fingerprint: String,
        import: bool,
    },
    Proceed,
    Stop,
    GroupMembers {
        selected: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum QuestionKey {
    Conflict { first: String, second: String },
    SelectProvider { depend: String },
    Replace { old: String, new: String },
    InstallIgnorepkg { name: String },
    RemovePkgs { names: Vec<String> },
    Corrupted { path: String },
    ImportKey { fingerprint: String },
    Proceed,
    GroupMembers { group: String },
}

impl Question {
    pub fn key(&self) -> QuestionKey {
        match self {
            Question::Conflict {
                incoming,
                removable,
            } => QuestionKey::Conflict {
                first: incoming.min(removable).clone(),
                second: incoming.max(removable).clone(),
            },
            Question::SelectProvider { depend, .. } => QuestionKey::SelectProvider {
                depend: depend.clone(),
            },
            Question::Replace { old, new, .. } => QuestionKey::Replace {
                old: old.clone(),
                new: new.clone(),
            },
            Question::InstallIgnorepkg { name } => {
                QuestionKey::InstallIgnorepkg { name: name.clone() }
            }
            Question::RemovePkgs { names } => {
                let mut ordered = names.clone();
                ordered.sort();
                QuestionKey::RemovePkgs { names: ordered }
            }
            Question::Corrupted { path } => QuestionKey::Corrupted { path: path.clone() },
            Question::ImportKey { fingerprint, .. } => QuestionKey::ImportKey {
                fingerprint: fingerprint.clone(),
            },
            Question::Proceed(_) => QuestionKey::Proceed,
            Question::GroupMembers { group, .. } => QuestionKey::GroupMembers {
                group: group.clone(),
            },
        }
    }

    pub fn is_runtime(&self) -> bool {
        matches!(
            self,
            Question::Corrupted { .. } | Question::ImportKey { .. }
        )
    }

    pub fn is_collectable(&self) -> bool {
        if self.is_runtime() {
            return false;
        }
        !matches!(self, Question::Proceed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    #[test]
    fn question_keys_normalize_order() {
        let forward = Question::Conflict {
            incoming: "cava-git".to_string(),
            removable: "cava".to_string(),
        };
        let backward = Question::Conflict {
            incoming: "cava".to_string(),
            removable: "cava-git".to_string(),
        };
        let unrelated = Question::Conflict {
            incoming: "nginx-mainline".to_string(),
            removable: "nginx".to_string(),
        };
        assert_eq!(forward.key(), backward.key());
        assert_eq!(
            forward.key(),
            QuestionKey::Conflict {
                first: "cava".to_string(),
                second: "cava-git".to_string(),
            }
        );
        assert_ne!(forward.key(), unrelated.key());
        assert_ne!(
            forward.key(),
            QuestionKey::SelectProvider {
                depend: "cava".to_string(),
            }
        );
        let shuffled = Question::RemovePkgs {
            names: vec!["b".to_string(), "a".to_string()],
        };
        assert_eq!(
            shuffled.key(),
            QuestionKey::RemovePkgs {
                names: vec!["a".to_string(), "b".to_string()],
            }
        );
    }

    #[test]
    fn collectable_questions_exclude_runtime_and_proceed() {
        let collectable = [
            Question::Conflict {
                incoming: "a".to_string(),
                removable: "b".to_string(),
            },
            Question::SelectProvider {
                depend: "sdl".to_string(),
                candidates: vec![],
            },
            Question::Replace {
                old: "nginx".to_string(),
                new: "nginx-mainline".to_string(),
                repo: None,
            },
            Question::InstallIgnorepkg {
                name: "glibc".to_string(),
            },
            Question::RemovePkgs { names: vec![] },
            Question::GroupMembers {
                group: "base-devel".to_string(),
                members: vec![],
            },
        ];
        for question in &collectable {
            assert!(question.is_collectable());
            assert!(!question.is_runtime());
        }
        for question in [
            Question::Corrupted {
                path: "p".to_string(),
            },
            Question::ImportKey {
                fingerprint: "f".to_string(),
                uid: "u".to_string(),
            },
        ] {
            assert!(question.is_runtime());
            assert!(!question.is_collectable());
        }
        assert!(!Question::Proceed(summary()).is_collectable());
        assert!(!Question::Proceed(summary()).is_runtime());
    }
}
