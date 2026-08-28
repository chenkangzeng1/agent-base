//! Execution-related pure types: FinishReason, RunOutcome.

use serde::{Deserialize, Serialize};

/// Semantic finish reason, replacing scattered `Option<String>` matching.
///
/// Each variant captures a distinct end-of-turn signal from the LLM provider.
/// Conversion helpers ([`FinishReason::from_openai`], [`FinishReason::from_anthropic`],
/// [`FinishReason::from_responses`]) normalise the provider-specific strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// Model finished naturally (OpenAI `"stop"` / Anthropic `"end_turn"`).
    Stop,
    /// Model requested tool calls (Anthropic `"tool_use"`).
    ///
    /// OpenAI's `"tool_calls"` is consumed at the client layer as
    /// `StreamChunk::ToolCall` and never reaches the react loop.
    ToolUse,
    /// Output was truncated by the token limit.
    ///
    /// - OpenAI: `"length"`
    /// - Anthropic: `"max_tokens"`
    /// - Responses API: `"incomplete"`
    Truncated {
        /// Provider-specific reason string, if available.
        reason: Option<String>,
    },
    /// Any other / unknown finish reason.
    Other(String),
}

impl FinishReason {
    /// Normalise an OpenAI Chat Completions `finish_reason` value.
    pub fn from_openai(s: Option<&str>) -> Self {
        match s {
            Some("stop") => Self::Stop,
            Some("length") => Self::Truncated {
                reason: Some("length".into()),
            },
            // "tool_calls" is consumed client-side, never reaches the react loop.
            Some(other) => Self::Other(other.to_string()),
            None => Self::Stop, // stream ended normally
        }
    }

    /// Normalise an Anthropic `stop_reason` value.
    pub fn from_anthropic(s: Option<&str>) -> Self {
        match s {
            Some("end_turn") => Self::Stop,
            Some("tool_use") => Self::ToolUse,
            Some("max_tokens") => Self::Truncated {
                reason: Some("max_tokens".into()),
            },
            Some(other) => Self::Other(other.to_string()),
            None => Self::Stop,
        }
    }

    /// Normalise an OpenAI Responses API status / incomplete reason.
    pub fn from_responses(reason: Option<&str>) -> Self {
        match reason {
            None => Self::Stop,
            Some("incomplete") => Self::Truncated { reason: None },
            Some(other) => Self::Other(other.to_string()),
        }
    }

    /// Unified conversion when the provider is unknown at compile time.
    ///
    /// Handles the union of OpenAI Chat and Anthropic stop-reason strings,
    /// as well as the Responses API `"incomplete:reason"` format.
    /// OpenAI `"tool_calls"` is consumed client-side and never reaches here,
    /// so it is not matched.
    pub fn from_raw(s: Option<&str>) -> Self {
        match s {
            Some("stop") | Some("end_turn") => Self::Stop,
            Some("tool_use") => Self::ToolUse,
            Some("length") => Self::Truncated {
                reason: Some("length".into()),
            },
            Some("max_tokens") => Self::Truncated {
                reason: Some("max_tokens".into()),
            },
            // Responses API: only "incomplete:max_output_tokens" is truncation;
            // "incomplete:content_filter" is a safety refusal, not a token limit.
            Some("incomplete:max_output_tokens") => Self::Truncated {
                reason: Some("incomplete:max_output_tokens".into()),
            },
            Some(s) if s.starts_with("incomplete:") => Self::Other(s.to_string()),
            Some(other) => Self::Other(other.to_string()),
            None => Self::Stop,
        }
    }

    /// Returns `true` if the response was truncated by the token limit.
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}

/// The outcome of an Agent turn or run.
///
/// Represents the state after a turn completes:
/// - `Completed` — task finished successfully; run ends.
/// - `Continuing` — turn ended but the run is still in progress (guard nudge).
/// - `Failed` — unrecoverable error; run ends.
/// - `MaxTurnsExceeded` — hit the turn cap; run ends.
/// - `Cancelled` — user or system cancelled; run ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    Completed,
    Continuing,
    Failed { error: String },
    MaxTurnsExceeded { turns: u32 },
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_stop() {
        assert_eq!(FinishReason::from_openai(Some("stop")), FinishReason::Stop);
    }

    #[test]
    fn openai_length() {
        assert_eq!(
            FinishReason::from_openai(Some("length")),
            FinishReason::Truncated {
                reason: Some("length".into())
            }
        );
    }

    #[test]
    fn openai_none() {
        assert_eq!(FinishReason::from_openai(None), FinishReason::Stop);
    }

    #[test]
    fn openai_other() {
        assert_eq!(
            FinishReason::from_openai(Some("content_filter")),
            FinishReason::Other("content_filter".into())
        );
    }

    #[test]
    fn anthropic_end_turn() {
        assert_eq!(
            FinishReason::from_anthropic(Some("end_turn")),
            FinishReason::Stop
        );
    }

    #[test]
    fn anthropic_tool_use() {
        assert_eq!(
            FinishReason::from_anthropic(Some("tool_use")),
            FinishReason::ToolUse
        );
    }

    #[test]
    fn anthropic_max_tokens() {
        assert_eq!(
            FinishReason::from_anthropic(Some("max_tokens")),
            FinishReason::Truncated {
                reason: Some("max_tokens".into())
            }
        );
    }

    #[test]
    fn responses_incomplete() {
        assert_eq!(
            FinishReason::from_responses(Some("incomplete")),
            FinishReason::Truncated { reason: None }
        );
    }

    #[test]
    fn responses_none() {
        assert_eq!(FinishReason::from_responses(None), FinishReason::Stop);
    }

    #[test]
    fn is_truncated_true() {
        assert!(FinishReason::Truncated { reason: None }.is_truncated());
        assert!(
            FinishReason::Truncated {
                reason: Some("length".into())
            }
            .is_truncated()
        );
    }

    #[test]
    fn is_truncated_false() {
        assert!(!FinishReason::Stop.is_truncated());
        assert!(!FinishReason::ToolUse.is_truncated());
        assert!(!FinishReason::Other("x".into()).is_truncated());
    }

    #[test]
    fn from_raw_incomplete_prefix() {
        assert_eq!(
            FinishReason::from_raw(Some("incomplete:max_output_tokens")),
            FinishReason::Truncated {
                reason: Some("incomplete:max_output_tokens".into())
            }
        );
        // content_filter is a safety refusal, not truncation
        assert_eq!(
            FinishReason::from_raw(Some("incomplete:content_filter")),
            FinishReason::Other("incomplete:content_filter".into())
        );
    }
}

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn from_openai_never_panics(s in proptest::option::of(".*")) {
            let result = FinishReason::from_openai(s.as_deref());
            // Verify it returns a valid variant
            match result {
                FinishReason::Stop | FinishReason::ToolUse |
                FinishReason::Truncated { .. } | FinishReason::Other(_) => {}
            }
        }

        #[test]
        fn from_anthropic_never_panics(s in proptest::option::of(".*")) {
            let result = FinishReason::from_anthropic(s.as_deref());
            match result {
                FinishReason::Stop | FinishReason::ToolUse |
                FinishReason::Truncated { .. } | FinishReason::Other(_) => {}
            }
        }

        #[test]
        fn from_raw_never_panics(s in proptest::option::of(".*")) {
            let result = FinishReason::from_raw(s.as_deref());
            match result {
                FinishReason::Stop | FinishReason::ToolUse |
                FinishReason::Truncated { .. } | FinishReason::Other(_) => {}
            }
        }

        #[test]
        fn from_responses_never_panics(s in proptest::option::of(".*")) {
            let result = FinishReason::from_responses(s.as_deref());
            match result {
                FinishReason::Stop | FinishReason::ToolUse |
                FinishReason::Truncated { .. } | FinishReason::Other(_) => {}
            }
        }

        #[test]
        fn truncated_is_always_truncated(reason in proptest::option::of(".*")) {
            let fr = FinishReason::Truncated { reason };
            assert!(fr.is_truncated());
        }
    }
}
