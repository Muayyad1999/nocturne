//! Why a design could not be produced.

use serde::{Deserialize, Serialize};

/// A design was not produced.
///
/// [`Requirements`](crate::Requirements) that cannot be built report *every*
/// problem at once, in the same words as [`validate`](crate::validate), so a
/// person fixes them in one pass instead of one error per attempt.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignError {
    /// The requirements themselves are the problem.
    #[error("these requirements cannot be built:\n  - {}", .0.join("\n  - "))]
    Requirements(Vec<String>),
    /// The requirements were acceptable but the design could not be completed.
    /// A bug in this crate, or a limit it does not state up front.
    #[error("the design could not be completed: {0}")]
    Internal(String),
}

impl DesignError {
    /// The problems, one per line, without the surrounding sentence.
    pub fn problems(&self) -> Vec<String> {
        match self {
            DesignError::Requirements(problems) => problems.clone(),
            DesignError::Internal(message) => vec![message.clone()],
        }
    }
}
