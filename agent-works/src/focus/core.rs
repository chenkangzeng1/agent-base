use std::sync::Arc;
use std::time::Duration;

use agent_base::{ChatMessage, LlmClient, ResponseFormat};
use serde::de::DeserializeOwned;
use serde_json::Value;

// ── FocusInput ───────────────────────────────────────────────────────────────

/// Input for a Focus call — either a simple string or a structured context.
pub trait FocusInput {
    /// Format the input into the user prompt text sent to the LLM.
    fn to_prompt(&self) -> String;
}

/// Simple case: pass a string directly.
impl FocusInput for str {
    fn to_prompt(&self) -> String {
        self.to_string()
    }
}

impl FocusInput for String {
    fn to_prompt(&self) -> String {
        self.clone()
    }
}

// ── Context ──────────────────────────────────────────────────────────────────

/// Structured context for multi-field input scenarios.
///
/// Fields are formatted as `【key】\nvalue` when sent to the LLM,
/// where the key acts as a label to help the LLM understand the context.
///
/// # Usage
///
/// ```ignore
/// let ctx = Context::new()
///     .add("command", "apt install nginx")
///     .add("screen", screen_content);
/// ```
pub struct Context {
    entries: Vec<(String, String)>,
}

impl Context {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a context field. The key is used as a label when sent to the LLM.
    pub fn add(mut self, key: &str, value: &str) -> Self {
        self.entries.push((key.to_string(), value.to_string()));
        self
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusInput for Context {
    fn to_prompt(&self) -> String {
        self.entries
            .iter()
            .map(|(key, value)| format!("【{}】\n{}", key, value))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

// ── FocusOutput ──────────────────────────────────────────────────────────────

/// Output wrapper for a Focus call.
///
/// Contains both the structured result and the raw LLM response,
/// useful for debugging when something goes wrong.
pub struct FocusOutput<T> {
    /// Deserialized structured result.
    pub result: T,
    /// Raw LLM response text (JSON string), for logging and debugging.
    pub raw_response: String,
}

// ── Focus ────────────────────────────────────────────────────────────────────

/// A focused LLM call.
///
/// Each instance is bound to a system prompt and dedicated to one specific
/// judgment question. Use `ask()` to send input and receive a structured
/// JSON answer.
///
/// # Usage
///
/// ```ignore
/// // Simple case: single string input
/// let classify = Focus::new(client, "You are a task complexity classifier...");
/// let output = classify.ask::<TaskComplexity>(&user_input, 5s).await?;
///
/// // Complex case: multiple context fields
/// let status_focus = Focus::new(client, "You are a task status judge...");
/// let ctx = Context::new()
///     .add("command", command)
///     .add("screen", screen);
/// let output = status_focus.ask::<TaskStatus>(&ctx, 5s).await?;
/// ```
pub struct Focus {
    client: Arc<dyn LlmClient>,
    system_prompt: String,
}

impl std::fmt::Debug for Focus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Focus").finish_non_exhaustive()
    }
}

impl Focus {
    /// Create a new Focus instance.
    ///
    /// - `client`: LLM client (shared; multiple Focus instances can reuse the same client)
    /// - `system_prompt`: The role and judgment rules for this Focus (bound at creation, never changes)
    pub fn new(client: Arc<dyn LlmClient>, system_prompt: impl Into<String>) -> Self {
        Self {
            client,
            system_prompt: system_prompt.into(),
        }
    }

    /// Make a focused LLM call.
    ///
    /// Sends the system prompt (bound at creation) + user input (this call),
    /// forces JSON output, and deserializes into `T`.
    ///
    /// # Arguments
    /// - `input`: User input — can be `&str` or `Context`
    /// - `timeout`: Call timeout
    ///
    /// # Returns
    /// `FocusOutput<T>` containing the structured result and raw response.
    pub async fn ask<T: DeserializeOwned>(
        &self,
        input: &impl FocusInput,
        timeout: Duration,
    ) -> Result<FocusOutput<T>, FocusError> {
        let user_prompt = input.to_prompt();

        // Logging: first line of prompt + char count (privacy-friendly)
        let prompt_first_line = user_prompt.lines().next().unwrap_or("(empty)");
        let prompt_char_count = user_prompt.chars().count();
        let sys_first_line = self.system_prompt.lines().next().unwrap_or("(empty)");
        let target_type = std::any::type_name::<T>();

        tracing::info!(
            target_type = target_type,
            system_prompt = %sys_first_line,
            user_prompt_first_line = %prompt_first_line,
            user_prompt_chars = prompt_char_count,
            timeout_secs = timeout.as_secs(),
            "[Focus] calling LLM"
        );

        let start = std::time::Instant::now();
        let messages = vec![
            ChatMessage::system(self.system_prompt.clone()),
            ChatMessage::user(user_prompt),
        ];

        let response = tokio::time::timeout(
            timeout,
            self.client
                .chat(&messages, &[], None, Some(&ResponseFormat::JsonObject)),
        )
        .await
        .map_err(|_| FocusError::Timeout(timeout))?
        .map_err(|e| FocusError::Llm(e.to_string()))?;

        let elapsed_ms = start.elapsed().as_millis();
        let raw_response = extract_content(&response).to_string();

        let result: T = serde_json::from_str(&raw_response).map_err(|e| {
            tracing::warn!(
                error = %e,
                raw_response = %raw_response,
                elapsed_ms = elapsed_ms,
                "[Focus] failed to parse LLM response as JSON"
            );
            FocusError::Parse {
                error: e.to_string(),
                raw: raw_response.clone(),
            }
        })?;

        tracing::info!(
            target_type = target_type,
            raw_response_chars = raw_response.chars().count(),
            elapsed_ms = elapsed_ms,
            "[Focus] call succeeded"
        );

        Ok(FocusOutput {
            result,
            raw_response,
        })
    }
}

// ── FocusError ───────────────────────────────────────────────────────────────

/// Error type for Focus calls.
#[derive(Debug)]
pub enum FocusError {
    /// LLM call timed out.
    Timeout(Duration),
    /// LLM call failed (network error, API error, etc.).
    Llm(String),
    /// LLM response could not be parsed into the expected JSON type.
    Parse { error: String, raw: String },
}

impl std::fmt::Display for FocusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FocusError::Timeout(d) => write!(f, "Focus timeout after {:?}", d),
            FocusError::Llm(e) => write!(f, "Focus LLM error: {}", e),
            FocusError::Parse { error, .. } => write!(f, "Focus parse error: {}", error),
        }
    }
}

impl std::error::Error for FocusError {}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Extract the `content` field from an LLM response.
///
/// Supports OpenAI-compatible format: `choices[0].message.content`.
/// Falls back to the full response string if extraction fails.
fn extract_content(response: &Value) -> &str {
    response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| {
            tracing::warn!(
                response = %response,
                "Focus: could not extract choices[0].message.content, using full response"
            );
            response.as_str().unwrap_or("{}")
        })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    // ── Context tests ──

    #[test]
    fn context_single_field() {
        let ctx = Context::new().add("command", "df -h");
        assert_eq!(ctx.to_prompt(), "【command】\ndf -h");
    }

    #[test]
    fn context_multiple_fields() {
        let ctx = Context::new()
            .add("command", "apt install nginx")
            .add("elapsed", "30s")
            .add("screen", "Reading package lists...");
        let expected = "【command】\napt install nginx\n\n【elapsed】\n30s\n\n【screen】\nReading package lists...";
        assert_eq!(ctx.to_prompt(), expected);
    }

    #[test]
    fn context_empty() {
        let ctx = Context::new();
        assert_eq!(ctx.to_prompt(), "");
    }

    // ── FocusInput tests ──

    #[test]
    fn str_input() {
        let input: &str = "hello";
        assert_eq!(input.to_prompt(), "hello");
    }

    #[test]
    fn string_input() {
        let input = String::from("hello");
        assert_eq!(input.to_prompt(), "hello");
    }

    // ── extract_content tests ──

    #[derive(Deserialize, Debug, PartialEq)]
    struct MockResult {
        status: String,
        reason: String,
    }

    #[test]
    fn extract_content_openai_format() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "{\"status\": \"finished\"}"
                }
            }]
        });
        assert_eq!(extract_content(&response), "{\"status\": \"finished\"}");
    }

    #[test]
    fn extract_content_missing_choices() {
        let response = serde_json::json!({"error": "something"});
        assert_eq!(extract_content(&response), "{}");
    }

    #[test]
    fn extract_content_empty_choices() {
        let response = serde_json::json!({"choices": []});
        assert_eq!(extract_content(&response), "{}");
    }

    #[test]
    fn focus_output_deserialize() {
        let raw = r#"{"status":"finished","reason":"done"}"#;
        let result: MockResult = serde_json::from_str(raw).unwrap();
        assert_eq!(result.status, "finished");
        assert_eq!(result.reason, "done");
    }

    // ── FocusError tests ──

    #[test]
    fn focus_error_display() {
        let err = FocusError::Timeout(Duration::from_secs(5));
        assert_eq!(format!("{}", err), "Focus timeout after 5s");

        let err = FocusError::Llm("network error".to_string());
        assert_eq!(format!("{}", err), "Focus LLM error: network error");

        let err = FocusError::Parse {
            error: "unexpected token".to_string(),
            raw: "not json".to_string(),
        };
        assert_eq!(format!("{}", err), "Focus parse error: unexpected token");
    }
}
