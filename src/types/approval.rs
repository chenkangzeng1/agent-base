use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RiskLevel {
    Safe,
    Sensitive,
    Destructive,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApprovalRequest {
    pub title: String,
    pub message: String,
    pub action_key: Option<String>,
    pub risk_level: RiskLevel,
    pub raw: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}
