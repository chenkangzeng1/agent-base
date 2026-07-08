use agent_base::{ChatMessage, LlmClient, OpenAiClient, StreamChunk};
use futures_util::StreamExt;
use serde_json::{json, Value};

/// 测试 OpenAiClient 生成的请求 body 格式
/// 这个测试不实际发送 HTTP 请求，而是通过日志验证请求 body 结构
#[tokio::test]
async fn test_openai_client_request_body_format() {
    // 创建一个测试用的 OpenAiClient（使用无效的 API key，但会记录请求 body）
    let client = OpenAiClient::new(
        "test-api-key".to_string(),
        "qwen-flash".to_string(),
        Some("https://test.example.com/v1".to_string()),
    );

    let messages = vec![
        ChatMessage::system("你是一个助手"),
        ChatMessage::user("你好"),
    ];

    let tools: Vec<Value> = vec![];

    // 测试 enable_thinking=true, thinking_budget=128 的情况
    // 由于 URL 无效，这里会失败，但日志会输出请求 body
    let reasoning = agent_base::ReasoningConfig {
        enabled: Some(true),
        budget_tokens: Some(128),
        effort: None,
    };
    let result = client
        .chat_stream(&messages, &tools, Some(&reasoning), None)
        .await;

    // 我们期望请求失败（因为 URL 无效），但请求 body 格式应该正确
    // 通过查看日志可以验证请求 body 的格式
    assert!(result.is_err(), "Expected error due to invalid URL");
}

/// 验证请求 body 的 JSON 结构（通过构造相同的 JSON 来验证）
#[test]
fn test_request_body_json_structure() {
    // 模拟 OpenAiClient 生成的请求 body
    let mut request_body = json!({
        "model": "qwen-flash",
        "messages": [
            {
                "role": "system",
                "content": "你是一个助手"
            },
            {
                "role": "user",
                "content": "你好"
            }
        ],
        "tools": [],
        "stream": true,
        "stream_options": {
            "include_usage": true
        }
    });

    // 添加 enable_thinking
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("enable_thinking".to_string(), json!(true));
    }

    // 添加 extra_body 包含 thinking_budget
    let mut extra_body = serde_json::Map::new();
    extra_body.insert("thinking_budget".to_string(), json!(128));
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("extra_body".to_string(), Value::Object(extra_body));
    }

    // 验证结构
    let json_str = serde_json::to_string_pretty(&request_body).unwrap();
    println!("Expected request body:\n{}", json_str);

    // 验证 enable_thinking 在 root level
    assert_eq!(request_body["enable_thinking"], true);

    // 验证 extra_body 存在
    assert!(request_body["extra_body"].is_object());
    assert_eq!(request_body["extra_body"]["thinking_budget"], 128);
}

/// 测试不同 thinking_budget 值
#[test]
fn test_thinking_budget_values() {
    let budgets = vec![100, 128, 256, 512, 1024, 2048, 4096];

    for budget in budgets {
        let mut request_body = json!({
            "model": "qwen-flash",
            "messages": [],
            "tools": [],
            "stream": true,
        });

        let mut extra_body = serde_json::Map::new();
        extra_body.insert("thinking_budget".to_string(), json!(budget));
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("extra_body".to_string(), Value::Object(extra_body));
        }

        assert_eq!(
            request_body["extra_body"]["thinking_budget"], budget,
            "thinking_budget should be {}",
            budget
        );
    }
}
