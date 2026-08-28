//! Checkpoint-related pure types: CheckpointData, CheckpointStep, ToolResultData.

use llm_trait::message::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::SessionId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointData {
    pub session_id: SessionId,
    pub user_input: String,
    pub step: CheckpointStep,
    pub turn_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckpointStep {
    AfterUserInput,
    BeforeLlm {
        messages: Vec<ChatMessage>,
        tools: Vec<Value>,
    },
    BeforeToolCalls {
        tool_calls: Vec<(String, String, String)>,
    },
    AfterToolCalls {
        tool_calls: Vec<(String, String, String)>,
        results: Vec<ToolResultData>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultData {
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("You are a helpful agent."),
            ChatMessage::user("What is the weather in Tokyo?"),
        ]
    }

    fn sample_tools() -> Vec<Value> {
        vec![serde_json::json!({"name": "weather", "args": {"city": "Tokyo"}})]
    }

    fn sample_tool_calls() -> Vec<(String, String, String)> {
        vec![
            (
                "call_1".to_string(),
                "weather".to_string(),
                "{\"city\": \"Tokyo\"}".to_string(),
            ),
            (
                "call_2".to_string(),
                "time".to_string(),
                "{\"tz\": \"Asia/Tokyo\"}".to_string(),
            ),
        ]
    }

    fn sample_results() -> Vec<ToolResultData> {
        vec![
            ToolResultData {
                tool_call_id: "call_1".to_string(),
                tool_name: "weather".to_string(),
                summary: "20C, clear".to_string(),
            },
            ToolResultData {
                tool_call_id: "call_2".to_string(),
                tool_name: "time".to_string(),
                summary: "09:41 JST".to_string(),
            },
        ]
    }

    fn checkpoint_data(step: CheckpointStep) -> CheckpointData {
        CheckpointData {
            session_id: SessionId::with_external_id(42, "test-session"),
            user_input: "What is the weather in Tokyo?".to_string(),
            step,
            turn_count: 3,
        }
    }

    #[test]
    fn checkpoint_step_after_user_input_round_trips() {
        let step = CheckpointStep::AfterUserInput;
        let json = serde_json::to_string(&step).unwrap();
        let decoded: CheckpointStep = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, CheckpointStep::AfterUserInput));
    }

    #[test]
    fn checkpoint_step_before_llm_round_trips() {
        let step = CheckpointStep::BeforeLlm {
            messages: sample_messages(),
            tools: sample_tools(),
        };
        let json = serde_json::to_string(&step).unwrap();
        let decoded: CheckpointStep = serde_json::from_str(&json).unwrap();
        match decoded {
            CheckpointStep::BeforeLlm { messages, tools } => {
                // ChatMessage has no PartialEq, so compare canonical JSON.
                assert_eq!(
                    serde_json::to_string(&messages).unwrap(),
                    serde_json::to_string(&sample_messages()).unwrap()
                );
                assert_eq!(tools, sample_tools());
            }
            other => panic!("expected BeforeLlm, got {other:?}"),
        }
    }

    #[test]
    fn checkpoint_step_before_tool_calls_round_trips() {
        let step = CheckpointStep::BeforeToolCalls {
            tool_calls: sample_tool_calls(),
        };
        let json = serde_json::to_string(&step).unwrap();
        let decoded: CheckpointStep = serde_json::from_str(&json).unwrap();
        match decoded {
            CheckpointStep::BeforeToolCalls { tool_calls } => {
                assert_eq!(tool_calls, sample_tool_calls());
            }
            other => panic!("expected BeforeToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn checkpoint_step_after_tool_calls_round_trips() {
        let step = CheckpointStep::AfterToolCalls {
            tool_calls: sample_tool_calls(),
            results: sample_results(),
        };
        let json = serde_json::to_string(&step).unwrap();
        let decoded: CheckpointStep = serde_json::from_str(&json).unwrap();
        match decoded {
            CheckpointStep::AfterToolCalls {
                tool_calls,
                results,
            } => {
                assert_eq!(tool_calls, sample_tool_calls());
                assert_eq!(results.len(), 2);
                assert_eq!(results[0].tool_call_id, "call_1");
                assert_eq!(results[0].tool_name, "weather");
                assert_eq!(results[0].summary, "20C, clear");
                assert_eq!(results[1].tool_call_id, "call_2");
                assert_eq!(results[1].tool_name, "time");
                assert_eq!(results[1].summary, "09:41 JST");
            }
            other => panic!("expected AfterToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn checkpoint_data_round_trips_for_every_step_variant() {
        let variants = [
            CheckpointStep::AfterUserInput,
            CheckpointStep::BeforeLlm {
                messages: sample_messages(),
                tools: sample_tools(),
            },
            CheckpointStep::BeforeToolCalls {
                tool_calls: sample_tool_calls(),
            },
            CheckpointStep::AfterToolCalls {
                tool_calls: sample_tool_calls(),
                results: sample_results(),
            },
        ];

        for step in variants {
            let data = checkpoint_data(step);
            let json = serde_json::to_string(&data).unwrap();
            let decoded: CheckpointData = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.session_id, data.session_id);
            assert_eq!(decoded.user_input, data.user_input);
            assert_eq!(decoded.turn_count, data.turn_count);
            // CheckpointStep does not derive PartialEq, so compare variant
            // discriminants for the step round-trip.
            assert_eq!(
                std::mem::discriminant(&data.step),
                std::mem::discriminant(&decoded.step)
            );
        }
    }

    #[test]
    fn checkpoint_step_deserialization_fails_gracefully_on_malformed_input() {
        // Unknown variant tag.
        assert!(serde_json::from_str::<CheckpointStep>(r#"{"NotAStep":{}}"#).is_err());
        // Wrong field type on a known variant.
        assert!(
            serde_json::from_str::<CheckpointStep>(
                r#"{"BeforeLlm":{"messages":"not-a-list","tools":[]}}"#,
            )
            .is_err()
        );
        // Truncated JSON.
        assert!(serde_json::from_str::<CheckpointStep>(r#"{"AfterUserInput""#).is_err());
        // Empty input.
        assert!(serde_json::from_str::<CheckpointStep>("").is_err());
    }

    #[test]
    fn checkpoint_data_deserialization_fails_gracefully_on_malformed_input() {
        // Missing required fields.
        assert!(
            serde_json::from_str::<CheckpointData>(r#"{"session_id":{"id":1},"user_input":"hi"}"#,)
                .is_err()
        );
        // Wrong session_id shape.
        assert!(
            serde_json::from_str::<CheckpointData>(
                r#"{"session_id":"oops","user_input":"hi","step":"AfterUserInput","turn_count":1}"#,
            )
            .is_err()
        );
        // turn_count is not a number.
        assert!(
            serde_json::from_str::<CheckpointData>(
                r#"{"session_id":{"id":1},"user_input":"hi","step":"AfterUserInput","turn_count":"three"}"#,
            )
            .is_err()
        );
    }
}
