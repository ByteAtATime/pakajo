use crate::tx::convert::PrepareFailure;

#[derive(Clone, Debug)]
pub enum ReviewError {
    Prepare(PrepareFailure),
    Other(String),
}

impl std::fmt::Display for ReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewError::Prepare(failure) => write!(f, "{failure}"),
            ReviewError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ReviewError {}

impl From<anyhow::Error> for ReviewError {
    fn from(error: anyhow::Error) -> Self {
        let found = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<PrepareFailure>().cloned());
        match found {
            Some(failure) => ReviewError::Prepare(failure),
            None => ReviewError::Other(format!("{error:#}")),
        }
    }
}
