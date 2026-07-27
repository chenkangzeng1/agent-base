use anyhow::{Result, anyhow};

const DEFAULT_MODEL: &str = "copilot";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Resolved LLM configuration.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    /// API key for the LLM provider.
    pub api_key: String,
    /// Model name (e.g. `"opus"`, `"gpt-4o"`).
    pub model: String,
    /// Base URL for the LLM API endpoint.
    pub base_url: String,
}

/// Resolve LLM configuration (API key, model, base_url).
///
/// Priority: CLI arg > environment variable (.env) > default
pub fn resolve_llm_config(model: Option<&str>, base_url: Option<&str>) -> Result<LlmConfig> {
    let api_key = super::optional_env("LLM_API_KEY")
        .or_else(|| super::optional_env("OPENAI_API_KEY"))
        .ok_or_else(|| anyhow!("Missing environment variable LLM_API_KEY. Please configure it in .env."))?;

    let resolved_model = model
        .map(|s| s.to_string())
        .or_else(|| super::optional_env("LLM_MODEL"))
        .or_else(|| super::optional_env("OPENAI_MODEL"))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let resolved_base_url = base_url
        .map(|s| s.to_string())
        .or_else(|| super::optional_env("LLM_BASE_URL"))
        .or_else(|| super::optional_env("OPENAI_BASE_URL"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    Ok(LlmConfig { api_key, model: resolved_model, base_url: resolved_base_url })
}
