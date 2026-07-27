//! Configuration resolution helpers.
//!
//! Handles LLM configuration (API key, model, base URL) with multi-source
//! resolution: CLI argument → environment variable (`.env`) → default.

pub mod llm;

pub use llm::{LlmConfig, resolve_llm_config};

use std::env;

/// Read an optional environment variable. Empty strings are treated as unset.
pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}
