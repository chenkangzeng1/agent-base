//! NDJSON protocol message types shared between phi-agent and language SDKs.
//!
//! This module has **zero** dependency on `agent-base` — it is a pure serde
//! contract.  SDK authors can use this file as the authoritative reference for
//! the wire format without pulling in the entire Rust crate.
//!
//! # Protocol overview
//!
//! - **Transport**: stdio, one JSON object per line (NDJSON).
//! - **Schema rule**: new fields may be added at any time (receivers MUST ignore
//!   unknown fields).  Removing or re-typing a field is a MAJOR version change.
//!
//! # Message flow
//!
//! ```text
//! SDK → phi serve         SDK ← phi serve
//! ─────────────────       ─────────────────
//! register_tool           hello (on connect)
//! create_session          session_created
//! run                     event
//! tool_result             tool_call
//! list_tools              tools_listed
//! cancel                  done
//!                         error
//! ```

use agent_base::ToolMetadata as AgentToolMetadata;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

// ── Tool metadata (bridge-facing, mirrors agent_base::ToolMetadata) ────

/// Stable wire-format representation of a registered tool's metadata.
/// Mirrors `agent_base::ToolMetadata` without depending on agent-base so
/// SDK authors can read this file as a pure serde contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub name: String,
    pub description: String,
    pub origin: String,
    pub version: String,
    pub requirements: Vec<String>,
}

impl From<AgentToolMetadata> for ToolMetadata {
    fn from(m: AgentToolMetadata) -> Self {
        Self {
            name: m.name,
            description: m.description,
            origin: m.origin,
            version: m.version,
            requirements: m.requirements,
        }
    }
}

// ── Incoming (SDK → phi serve) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncomingMessage {
    RegisterTool {
        name: String,
        description: String,
        parameters: Value,
    },
    CreateSession {
        #[serde(default)]
        session_id: Option<String>,
    },
    Run {
        #[serde(default)]
        session_id: String,
        query: String,
        #[serde(default)]
        config: Option<RunConfig>,
    },
    ToolResult {
        call_id: String,
        summary: String,
        #[serde(default)]
        raw: Option<Value>,
        #[serde(default)]
        control_flow: Option<String>,
    },
    Cancel {
        #[serde(default)]
        session_id: String,
    },
    ListTools {},
}

#[derive(Debug, Deserialize, Default)]
pub struct RunConfig {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    pub enable_thinking: Option<bool>,
    pub thinking_budget: Option<u64>,
    pub thinking_effort: Option<String>,
    pub max_tool_calls_per_turn: Option<usize>,
    pub max_consecutive_failures: Option<usize>,
    pub max_turns: Option<u32>,
}

// ── Outgoing (phi serve → SDK) ────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutgoingMessage {
    Hello {
        protocol_version: u32,
        server_name: String,
        server_version: String,
    },
    SessionCreated {
        session_id: Option<String>,
        internal_id: u64,
    },
    Event {
        seq: u64,
        #[serde(flatten)]
        event: Value,
    },
    ToolCall {
        seq: u64,
        call_id: String,
        name: String,
        args: Value,
    },
    ToolRegistered {
        name: String,
        ok: bool,
    },
    Done {
        seq: u64,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turns: Option<u32>,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
    ToolsListed {
        tools: Vec<ToolMetadata>,
    },
}
