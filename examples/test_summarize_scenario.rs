/// 模拟 ops-omni 的实际场景：模型已调完工具，拿到 df -h 结果，需要写文字总结。
///
/// 测试 qwen3.7-max 在"总结回复"场景下是否会产生 content。
use agent_base::{AgentResult, ChatMessage, StreamClient, OpenAiClient, StreamChunk};
use futures_util::StreamExt;

async fn test_summarize(
    client: &OpenAiClient,
    label: &str,
    enable_thinking: bool,
) -> AgentResult<()> {
    println!("\n{}", "=".repeat(70));
    println!("{} — thinking={}", label, enable_thinking);
    println!("{}", "=".repeat(70));

    // 模拟 ops-omni 的多轮对话：用户问 → 模型调工具 → 工具返回结果 → 模型需要总结
    let messages = vec![
        ChatMessage::system("你是运维工程师助手，回复简洁直接。"),
        ChatMessage::user("看下磁盘空间"),
        // 模型之前的回复（调了工具）
        ChatMessage::Assistant {
            content: None,
            reasoning_content: None,
            tool_calls: Some(vec![agent_base::ToolCallMessage {
                id: "call_001".to_string(),
                name: "execute_command".to_string(),
                arguments: r#"{"command":"df -h","target_host":"10.0.0.1"}"#.to_string(),
            }]),
        },
        // 工具返回的结果
        ChatMessage::Tool {
            tool_call_id: "call_001".to_string(),
            content: "Filesystem      Size  Used Avail Use% Mounted on\n/dev/vda3        40G  8.0G   30G  22% /\ntmpfs           1.8G     0  1.8G   0% /dev/shm\n/dev/vda2       197M  6.3M  191M   4% /boot/efi".to_string(),
        },
    ];

    let reasoning = agent_base::ReasoningConfig {
        enabled: Some(enable_thinking),
        budget_tokens: None,
        effort: None,
    };

    let mut stream = client
        .stream(
            &messages,
            &[] as &[serde_json::Value],
            Some(&reasoning),
            None,
        )
        .await?;

    let mut thought = String::new();
    let mut text = String::new();
    let mut tool_calls = 0;
    let mut has_stop = false;

    while let Some(chunk) = stream.next().await {
        match chunk? {
            StreamChunk::Thought(t) => {
                thought.push_str(&t);
            }
            StreamChunk::Text(t) => {
                text.push_str(&t);
            }
            StreamChunk::ToolCall(_) => {
                tool_calls += 1;
            }
            StreamChunk::Stop { .. } => {
                has_stop = true;
            }
            StreamChunk::Usage(u) => {
                println!(
                    "  📊 prompt={}, completion={}",
                    u.prompt_tokens.unwrap_or(0),
                    u.completion_tokens.unwrap_or(0)
                );
            }
        }
    }

    println!("  💭 思考: {} 字符", thought.len());
    if !thought.is_empty() {
        let preview: String = thought.chars().take(200).collect();
        println!("     {}", preview);
    }
    println!("  📝 回复: {} 字符", text.len());
    if !text.is_empty() {
        let preview: String = text.chars().take(300).collect();
        println!("     {}", preview);
    }
    println!("  🔧 工具调用: {} | 🏁 Stop: {}", tool_calls, has_stop);

    if text.is_empty() && tool_calls == 0 {
        println!("  ⚠️  空响应！react_loop 会重试");
    } else if text.is_empty() && !thought.is_empty() {
        println!("  ⚠️  有思考无正文（思考内容应该作为兜底输出）");
    } else {
        println!("  ✅ 正常");
    }

    Ok(())
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("DASHSCOPE_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .map_err(|_| agent_base::AgentError::internal("API key 未设置"))?;

    let base_url = std::env::var("DASHSCOPE_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());

    let models = vec![
        ("qwen-flash", "千问 Flash"),
        ("qwen-plus", "千问 Plus"),
        ("qwen3.7-max", "千问 Max"),
    ];

    for (model_id, model_label) in &models {
        let client = OpenAiClient::new(
            api_key.clone(),
            model_id.to_string(),
            Some(base_url.clone()),
        );
        test_summarize(&client, model_label, false).await?;
        test_summarize(&client, model_label, true).await?;
    }

    Ok(())
}
