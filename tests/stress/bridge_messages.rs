//! Bridge message format conformance tests.
//!
//! Validates the NDJSON wire-format contract defined in [`bridge::messages`].
//! These tests ensure that SDK authors can rely on message shapes being stable
//! and that receivers handle unknown fields gracefully (forward-compat).

use phi_agent::bridge::messages::{IncomingMessage, OutgoingMessage, PROTOCOL_VERSION, RunConfig, ToolMetadata};
use serde_json::json;

// ── IncomingMessage ─────────────────────────────────────────────────────

#[test]
fn test_incoming_register_tool_with_complex_parameters() {
    let json = serde_json::to_string(&json!({
        "type": "register_tool",
        "name": "search",
        "description": "Search the web",
        "parameters": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["query"]
        }
    }))
    .unwrap();
    let msg: IncomingMessage = serde_json::from_str(&json).unwrap();
    match msg {
        IncomingMessage::RegisterTool { name, description, parameters } => {
            assert_eq!(name, "search");
            assert_eq!(description, "Search the web");
            assert_eq!(parameters["properties"]["query"]["type"], "string");
            assert_eq!(parameters["properties"]["limit"]["minimum"], 1);
        },
        _ => panic!("expected RegisterTool"),
    }
}

#[test]
fn test_incoming_create_session_empty() {
    let msg: IncomingMessage = serde_json::from_str(r#"{"type":"create_session"}"#).unwrap();
    match msg {
        IncomingMessage::CreateSession { session_id } => assert!(session_id.is_none()),
        _ => panic!("expected CreateSession"),
    }
}

#[test]
fn test_incoming_create_session_with_id() {
    let msg: IncomingMessage = serde_json::from_str(r#"{"type":"create_session","session_id":"ext-abc-123"}"#).unwrap();
    match msg {
        IncomingMessage::CreateSession { session_id } => assert_eq!(session_id.unwrap(), "ext-abc-123"),
        _ => panic!("expected CreateSession"),
    }
}

#[test]
fn test_incoming_run_minimal() {
    let msg: IncomingMessage =
        serde_json::from_str(r#"{"type":"run","session_id":"s1","query":"hello world"}"#).unwrap();
    match msg {
        IncomingMessage::Run { session_id, query, config } => {
            assert_eq!(session_id, "s1");
            assert_eq!(query, "hello world");
            assert!(config.is_none());
        },
        _ => panic!("expected Run"),
    }
}

#[test]
fn test_incoming_run_with_partial_config() {
    let json = serde_json::to_string(&json!({
        "type": "run",
        "session_id": "s1",
        "query": "test",
        "config": {
            "model": "gpt-4",
            "max_turns": 3
        }
    }))
    .unwrap();
    let msg: IncomingMessage = serde_json::from_str(&json).unwrap();
    match msg {
        IncomingMessage::Run { config: Some(cfg), .. } => {
            assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
            assert_eq!(cfg.max_turns, Some(3));
            assert!(cfg.api_key.is_none());
            assert!(cfg.base_url.is_none());
        },
        _ => panic!("expected Run with config"),
    }
}

#[test]
fn test_incoming_run_default_session_id() {
    let json = r#"{"type":"run","query":"test"}"#;
    let msg: IncomingMessage = serde_json::from_str(json).unwrap();
    match msg {
        IncomingMessage::Run { session_id, .. } => assert_eq!(session_id, ""),
        _ => panic!("expected Run"),
    }
}

#[test]
fn test_incoming_tool_result_minimal() {
    let msg: IncomingMessage =
        serde_json::from_str(r#"{"type":"tool_result","call_id":"c1","summary":"done"}"#).unwrap();
    match msg {
        IncomingMessage::ToolResult { call_id, summary, raw, control_flow } => {
            assert_eq!(call_id, "c1");
            assert_eq!(summary, "done");
            assert!(raw.is_none());
            assert!(control_flow.is_none());
        },
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn test_incoming_tool_result_with_control_flow() {
    let json = r#"{"type":"tool_result","call_id":"c2","summary":"ok","control_flow":"break"}"#;
    let msg: IncomingMessage = serde_json::from_str(json).unwrap();
    match msg {
        IncomingMessage::ToolResult { control_flow, .. } => {
            assert_eq!(control_flow.unwrap(), "break");
        },
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn test_incoming_cancel() {
    let msg: IncomingMessage = serde_json::from_str(r#"{"type":"cancel","session_id":"abc"}"#).unwrap();
    match msg {
        IncomingMessage::Cancel { session_id } => assert_eq!(session_id, "abc"),
        _ => panic!("expected Cancel"),
    }
}

#[test]
fn test_incoming_cancel_default_session_id() {
    let msg: IncomingMessage = serde_json::from_str(r#"{"type":"cancel"}"#).unwrap();
    match msg {
        IncomingMessage::Cancel { session_id } => assert_eq!(session_id, ""),
        _ => panic!("expected Cancel"),
    }
}

#[test]
fn test_incoming_list_tools() {
    let msg: IncomingMessage = serde_json::from_str(r#"{"type":"list_tools"}"#).unwrap();
    assert!(matches!(msg, IncomingMessage::ListTools {}));
}

#[test]
fn test_incoming_unknown_type_is_error() {
    let result = serde_json::from_str::<IncomingMessage>(r#"{"type":"nonexistent","data":1}"#);
    assert!(result.is_err(), "unknown message type should be an error");
}

#[test]
fn test_incoming_missing_type_is_error() {
    let result = serde_json::from_str::<IncomingMessage>(r#"{"session_id":"abc"}"#);
    assert!(result.is_err(), "missing type field should be an error");
}

#[test]
fn test_incoming_extra_fields_ignored() {
    // Forward-compat: unknown fields must not cause deserialization errors.
    let json = serde_json::to_string(&json!({
        "type": "run",
        "session_id": "s1",
        "query": "test",
        "future_field_v2": { "nested": { "deep": true } },
        "another_new_field": "some-value"
    }))
    .unwrap();
    let msg: IncomingMessage = serde_json::from_str(&json).unwrap();
    assert!(matches!(msg, IncomingMessage::Run { .. }));
}

#[test]
fn test_incoming_run_config_extra_fields_ignored() {
    let json = serde_json::to_string(&json!({
        "type": "run",
        "session_id": "s1",
        "query": "test",
        "config": {
            "model": "gpt-4",
            "future_config_option": true
        }
    }))
    .unwrap();
    let msg: IncomingMessage = serde_json::from_str(&json).unwrap();
    match msg {
        IncomingMessage::Run { config: Some(cfg), .. } => {
            assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
        },
        _ => panic!("expected Run with config"),
    }
}

// ── OutgoingMessage ─────────────────────────────────────────────────────

#[test]
fn test_outgoing_hello_includes_protocol_version() {
    let msg = OutgoingMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        server_name: "phi".into(),
        server_version: "1.0.0".into(),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "hello");
    assert_eq!(json["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(json["server_name"], "phi");
    assert_eq!(json["server_version"], "1.0.0");
}

#[test]
fn test_outgoing_session_created_both_ids() {
    let msg = OutgoingMessage::SessionCreated { session_id: Some("ext-1".into()), internal_id: 42 };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["session_id"], "ext-1");
    assert_eq!(json["internal_id"], 42);
}

#[test]
fn test_outgoing_session_created_no_external_id() {
    let msg = OutgoingMessage::SessionCreated { session_id: None, internal_id: 7 };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["session_id"], serde_json::Value::Null);
    assert_eq!(json["internal_id"], 7);
}

#[test]
fn test_outgoing_event_flattens_inner_fields() {
    let msg = OutgoingMessage::Event {
        seq: 5,
        event: json!({"type": "text_delta", "text": "hello world", "agent_id": null}),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["seq"], 5);
    assert_eq!(json["text"], "hello world");
}

#[test]
fn test_outgoing_tool_call() {
    let msg = OutgoingMessage::ToolCall {
        seq: 3,
        call_id: "call-001".into(),
        name: "shell".into(),
        args: json!({"cmd": "ls -la", "timeout_ms": 5000}),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "tool_call");
    assert_eq!(json["seq"], 3);
    assert_eq!(json["call_id"], "call-001");
    assert_eq!(json["name"], "shell");
    assert_eq!(json["args"]["cmd"], "ls -la");
    assert_eq!(json["args"]["timeout_ms"], 5000);
}

#[test]
fn test_outgoing_tool_registered_success() {
    let msg = OutgoingMessage::ToolRegistered { name: "my_tool".into(), ok: true };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "tool_registered");
    assert!(json["ok"].as_bool().unwrap());
}

#[test]
fn test_outgoing_tool_registered_failure() {
    let msg = OutgoingMessage::ToolRegistered { name: "bad_tool".into(), ok: false };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["ok"], false);
}

#[test]
fn test_outgoing_done_completed() {
    let msg = OutgoingMessage::Done { seq: 10, outcome: "completed".into(), error: None, turns: Some(3) };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "done");
    assert_eq!(json["outcome"], "completed");
    assert_eq!(json["turns"], 3);
    // error field should NOT be present (skip_serializing_if)
    assert!(json.get("error").is_none());
}

#[test]
fn test_outgoing_done_with_error() {
    let msg = OutgoingMessage::Done {
        seq: 11,
        outcome: "failed".into(),
        error: Some("tool execution timeout".into()),
        turns: Some(1),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["error"], "tool execution timeout");
}

#[test]
fn test_outgoing_done_without_turns_field() {
    let msg = OutgoingMessage::Done { seq: 12, outcome: "cancelled".into(), error: None, turns: None };
    let json = serde_json::to_value(&msg).unwrap();
    assert!(json.get("error").is_none());
    assert!(json.get("turns").is_none());
}

#[test]
fn test_outgoing_error_minimal() {
    let msg = OutgoingMessage::Error { code: "E001".into(), message: "session not found".into(), detail: None };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["code"], "E001");
    assert_eq!(json["message"], "session not found");
    assert!(json.get("detail").is_none());
}

#[test]
fn test_outgoing_error_with_detail() {
    let msg = OutgoingMessage::Error {
        code: "E002".into(),
        message: "validation failed".into(),
        detail: Some(json!({"field": "session_id", "reason": "too long"})),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["detail"]["field"], "session_id");
}

#[test]
fn test_outgoing_tools_listed_empty() {
    let msg = OutgoingMessage::ToolsListed { tools: vec![] };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["type"], "tools_listed");
    assert_eq!(json["tools"].as_array().unwrap().len(), 0);
}

#[test]
fn test_outgoing_tools_listed_with_entries() {
    let tools = vec![
        ToolMetadata {
            name: "shell".into(),
            description: "Run shell".into(),
            origin: "phi-tools".into(),
            version: "1.0.0".into(),
            requirements: vec!["bash".into()],
        },
        ToolMetadata {
            name: "search".into(),
            description: "Search web".into(),
            origin: "phi-tools".into(),
            version: "1.0.0".into(),
            requirements: vec![],
        },
    ];
    let msg = OutgoingMessage::ToolsListed { tools };
    let json = serde_json::to_value(&msg).unwrap();
    let arr = json["tools"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "shell");
    assert_eq!(arr[1]["name"], "search");
}

// ── RunConfig ───────────────────────────────────────────────────────────

#[test]
fn test_run_config_all_fields_populated() {
    let json = json!({
        "model": "claude-sonnet-5",
        "api_key": "sk-test",
        "base_url": "https://api.example.com/v1",
        "enable_thinking": true,
        "thinking_budget": 64000,
        "thinking_effort": "xhigh",
        "max_tool_calls_per_turn": 20,
        "max_consecutive_failures": 5,
        "max_turns": 10
    });
    let cfg: RunConfig = serde_json::from_value(json).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(cfg.thinking_budget, Some(64000));
    assert_eq!(cfg.thinking_effort.as_deref(), Some("xhigh"));
    assert_eq!(cfg.max_tool_calls_per_turn, Some(20));
    assert_eq!(cfg.max_consecutive_failures, Some(5));
    assert_eq!(cfg.max_turns, Some(10));
}

#[test]
fn test_run_config_empty_is_all_none() {
    let cfg: RunConfig = serde_json::from_str("{}").unwrap();
    assert!(cfg.model.is_none());
    assert!(cfg.api_key.is_none());
    assert!(cfg.base_url.is_none());
    assert!(cfg.enable_thinking.is_none());
    assert!(cfg.thinking_budget.is_none());
    assert!(cfg.thinking_effort.is_none());
    assert!(cfg.max_tool_calls_per_turn.is_none());
    assert!(cfg.max_consecutive_failures.is_none());
    assert!(cfg.max_turns.is_none());
}

// ── ToolMetadata round-trip ─────────────────────────────────────────────

#[test]
fn test_tool_metadata_round_trip_empty_requirements() {
    let tm = ToolMetadata {
        name: "noop".into(),
        description: "does nothing".into(),
        origin: "test".into(),
        version: "0.1.0".into(),
        requirements: vec![],
    };
    let json = serde_json::to_string(&tm).unwrap();
    let back: ToolMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, tm.name);
    assert_eq!(back.description, tm.description);
    assert!(back.requirements.is_empty());
}

#[test]
fn test_tool_metadata_round_trip_with_requirements() {
    let tm = ToolMetadata {
        name: "browser".into(),
        description: "Browser automation".into(),
        origin: "phi-tools".into(),
        version: "2.0.0".into(),
        requirements: vec!["chrome>=120".into(), "chromedriver".into(), "network".into()],
    };
    let json = serde_json::to_string(&tm).unwrap();
    let back: ToolMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.requirements, vec!["chrome>=120", "chromedriver", "network"]);
}

// ── Serialization determinism ───────────────────────────────────────────

#[test]
fn test_outgoing_hello_is_deterministic() {
    let msg = OutgoingMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        server_name: "phi".into(),
        server_version: "1.0.0".into(),
    };
    let json1 = serde_json::to_string(&msg).unwrap();
    let json2 = serde_json::to_string(&msg).unwrap();
    assert_eq!(json1, json2);
}

#[test]
fn test_outgoing_done_is_deterministic() {
    let msg = OutgoingMessage::Done { seq: 1, outcome: "completed".into(), error: None, turns: Some(5) };
    let json1 = serde_json::to_string(&msg).unwrap();
    let json2 = serde_json::to_string(&msg).unwrap();
    assert_eq!(json1, json2);
}
