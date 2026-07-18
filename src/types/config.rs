use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    En,
    Zh,
}

impl Default for Language {
    fn default() -> Self {
        Language::En
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::En => write!(f, "en"),
            Language::Zh => write!(f, "zh"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub backoff_multiplier: f64,
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    pub fn initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    pub fn max_backoff_ms(mut self, ms: u64) -> Self {
        self.max_backoff_ms = ms;
        self
    }

    pub fn no_jitter(mut self) -> Self {
        self.jitter = false;
        self
    }
}

#[derive(Clone, Debug)]
pub enum ResponseFormat {
    JsonObject,
    JsonSchema {
        name: String,
        schema: Value,
    },
}

impl ResponseFormat {
    pub fn to_api_value(&self) -> Value {
        match self {
            ResponseFormat::JsonObject => {
                serde_json::json!({ "type": "json_object" })
            }
            ResponseFormat::JsonSchema { name, schema } => {
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": name,
                        "schema": schema,
                    }
                })
            }
        }
    }
}

use crate::llm::ReasoningConfig;

/// Safety configuration for agent runtime guardrails.
///
/// These limits are hard constraints enforced by code, not prompt —
/// the model cannot bypass them regardless of its capability.
#[derive(Clone, Debug)]
pub struct SafetyConfig {
    /// Maximum number of tool calls allowed per turn.
    /// When exceeded, tool calls are discarded and the LLM is forced to summarize.
    /// Default: 128.
    pub max_tool_calls_per_turn: usize,

    /// Maximum consecutive failures for the same tool before stopping retries.
    /// Default: 3.
    pub max_consecutive_failures: usize,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            max_tool_calls_per_turn: 128,
            max_consecutive_failures: 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
    /// Controls whether to include the reasoning content in LLM responses.
    ///
    /// Distinction from `reasoning.enabled`:
    /// - `enable_thought`: controls whether the `reasoning_content` field is forwarded
    ///   to consumers (i.e., "show the thinking process")
    /// - `reasoning.enabled`: controls whether the model's extended thinking / reasoning
    ///   mode is enabled (i.e., "let the model think deeply")
    ///
    /// Both are usually kept in sync, but can be controlled independently. For example,
    /// to enable deep thinking without showing the process, set `enable_thought = false`
    /// and `reasoning.enabled = true`.
    pub enable_thought: bool,
    /// Reasoning/thinking configuration that controls LLM reasoning behavior.
    ///
    /// - `enabled`: whether to enable extended thinking mode (equivalent to builder's `enable_thinking`)
    /// - `budget_tokens`: thinking token budget cap
    /// - `effort`: reasoning intensity/depth (semantics vary by provider)
    pub reasoning: Option<ReasoningConfig>,
    pub language: Language,
    pub execution: ExecutionConfig,
    pub llm: LlmConfig,
    pub tool: ToolConfig,
    pub session: SessionConfig,
    pub safety: SafetyConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            enable_thought: false,
            reasoning: None,
            language: Language::default(),
            execution: ExecutionConfig::default(),
            llm: LlmConfig::default(),
            tool: ToolConfig::default(),
            session: SessionConfig::default(),
            safety: SafetyConfig::default(),
        }
    }
}

impl AgentConfig {

    /// Validate the configuration, returning an error for invalid values.
    pub fn validate(&self) -> crate::types::AgentResult<()> {
        use crate::types::AgentError;

        if let Some(max_turns) = self.execution.max_turns {
            if max_turns == 0 {
                return Err(AgentError::config_error(
                    "execution.max_turns must be > 0".to_string(),
                ));
            }
        }

        if let Some(max_sessions) = self.session.max_sessions {
            if max_sessions == 0 {
                return Err(AgentError::config_error(
                    "session.max_sessions must be > 0".to_string(),
                ));
            }
        }

        if let Some(tool_timeout_ms) = self.tool.tool_timeout_ms {
            if tool_timeout_ms == 0 {
                return Err(AgentError::config_error(
                    "tool.tool_timeout_ms must be > 0".to_string(),
                ));
            }
        }

        if self.safety.max_tool_calls_per_turn == 0 {
            return Err(AgentError::config_error(
                "safety.max_tool_calls_per_turn must be > 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionConfig {
    pub max_turns: Option<u32>,
    pub approval_timeout_ms: Option<u64>,
    pub fail_on_persist_error: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            approval_timeout_ms: None,
            fail_on_persist_error: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub response_format: Option<ResponseFormat>,
    pub llm_retry: Option<RetryConfig>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            response_format: None,
            llm_retry: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolConfig {
    pub tool_timeout_ms: Option<u64>,
    pub max_tool_output_chars: Option<usize>,
    pub tool_error_retry_prompt: Option<String>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            tool_timeout_ms: None,
            max_tool_output_chars: None,
            tool_error_retry_prompt: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// 最大 session 数量，超过时 LRU 逐出整个 session（从内存卸载，数据保留）
    /// None = 不限制
    pub max_sessions: Option<usize>,

    /// 单个 session 最大保留轮数，超过从前面截掉最旧轮次
    /// None = 不限制
    pub max_turns_per_session: Option<usize>,

    /// 单条消息 token 上限（安全阀），超过不存入 session 历史
    /// 阈值应设很高（如 100k），只拦异常情况
    /// None = 不限制
    pub max_message_tokens: Option<usize>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_sessions: None,
            max_turns_per_session: None,
            max_message_tokens: None,
        }
    }
}
