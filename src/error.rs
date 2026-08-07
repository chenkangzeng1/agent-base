//! Internal error conversion helpers (Phase 1: eliminate anyhow).
//!
//! These convert `std::io::Error` and `serde_json::Error` into `AgentError`
//! variants. TODO(Phase 2): when agent-base gains native `From` impls, remove these.

use agent_base::AgentError;

/// Convert a `std::io::Error` to `AgentError::Internal`.
#[inline]
pub(crate) fn io_err(e: std::io::Error) -> AgentError {
    AgentError::internal(e.to_string())
}

/// Convert a `serde_json::Error` to `AgentError::Json`.
#[inline]
pub(crate) fn serde_err(e: serde_json::Error) -> AgentError {
    AgentError::json(e.to_string())
}
