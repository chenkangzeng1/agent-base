pub mod llm;

pub use llm::{resolve_llm_config, LlmConfig};

use std::env;

/// Read an optional environment variable. Empty strings are treated as unset.
pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}
