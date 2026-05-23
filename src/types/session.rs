use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SessionId {
    pub id: u64,
    pub external_id: Option<String>,
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
