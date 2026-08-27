//! Session-related pure types: SessionId.

use serde::{Deserialize, Serialize};

/// Unique identifier for an agent session.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SessionId {
    pub id: u64,
    pub external_id: Option<String>,
}

impl SessionId {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            external_id: None,
        }
    }

    pub fn with_external_id(id: u64, external_id: impl Into<String>) -> Self {
        Self {
            id,
            external_id: Some(external_id.into()),
        }
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref ext) = self.external_id {
            write!(f, "{}({})", self.id, ext)
        } else {
            write!(f, "{}", self.id)
        }
    }
}
