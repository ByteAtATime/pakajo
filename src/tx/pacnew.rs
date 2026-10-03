use serde::{Deserialize, Serialize};

use crate::events::MergeOrigin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeKind {
    Pacnew,
    Pacsave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeAction {
    KeepCurrent,
    TakeNew,
    Merge { hunks: Vec<usize> },
    Restore,
    Delete,
    Defer,
}

impl MergeOrigin {
    pub fn of(old: Option<&alpm::Package>, new: Option<&alpm::Package>) -> Option<MergeOrigin> {
        let package = new.or(old)?.name().to_string();
        Some(MergeOrigin {
            package,
            old_version: old.map(|p| p.version().to_string()),
            new_version: new.map(|p| p.version().to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_origin_of_nothing_is_nothing() {
        assert_eq!(MergeOrigin::of(None, None), None);
    }
}
