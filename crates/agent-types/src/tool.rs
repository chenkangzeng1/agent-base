//! Tool-related pure types: Content, ToolMetadata, ToolExposure, ActivationContext.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::session::SessionId;

/// Structured content returned by a tool, aligned with the MCP `content`
/// array shape (no envelope, no orchestration/failure/truncation semantics).
///
/// Only `Text` is consumed by the first LLM adapter; `Image` is shape-reserved
/// and the adapter reports "not supported" rather than silently dropping it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    /// Base64-encoded image payload.
    Image {
        data: String,
        mime_type: String,
    },
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text { text: s.into() }
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Content::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

impl From<Content> for Vec<Content> {
    fn from(c: Content) -> Self {
        vec![c]
    }
}

/// Join the textual portion of tool output into a single string for display
/// and session history. Non-text variants (e.g. `Image`) are skipped.
pub fn content_text(contents: &[Content]) -> String {
    contents
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            Content::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Machine-readable metadata for a registered tool — origin, version, and
/// runtime requirements in a stable shape consumers can inspect without
/// parsing the LLM-facing definition JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolMetadata {
    /// Tool name (matches `Tool::name`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Where this tool comes from: a crate name (e.g. `"phi-tools"`), a
    /// framework identifier (`"agent-base"`, `"agent-works"`), or
    /// `"custom"` for user-defined tools.
    pub origin: String,
    /// Crate / package version, or `"unknown"` when built outside a crate.
    pub version: String,
    /// Optional runtime requirements or capabilities this tool depends on.
    pub requirements: Vec<String>,
}

/// Visibility level of a tool to the LLM model.
///
/// Controls whether a tool appears in the tool definitions sent to the model
/// each turn. Defaults to `Direct` for backward compatibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolExposure {
    /// Always visible to the model. This is the default.
    Direct,
    /// Conditionally visible — the tool decides via `Tool::should_activate`.
    Deferred,
    /// Never visible to the model (internal/framework tools).
    Hidden,
}

/// Context passed to `Tool::should_activate` for Deferred tools.
///
/// Built once per react-loop iteration; tools inspect it to decide
/// whether they should be exposed to the model this turn.
#[derive(Clone, Debug)]
pub struct ActivationContext {
    /// Current session ID.
    pub session_id: SessionId,
    /// Names of tools already activated (Direct + activated Deferred) this turn.
    pub current_tools: Vec<String>,
    /// Workspace / working directory path.
    pub workspace: PathBuf,
}
