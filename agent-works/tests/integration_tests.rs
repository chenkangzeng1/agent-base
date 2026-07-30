use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentResult, ChatMessage, LlmCapabilities, LlmClient, ResponseFormat, RunOutcome, StreamChunk,
    Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use agent_works::AgentBuilder;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{Value, json};

type ChunkStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

struct MockLlmClient {
    responses: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
    call_count: Mutex<usize>,
}

impl MockLlmClient {
    fn new(scripted_responses: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(scripted_responses.into_iter()),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<ChunkStream> {
        *self.call_count.lock().unwrap() += 1;
        let chunks: Vec<AgentResult<StreamChunk>> = self
            .responses
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();
        let stream = futures_util::stream::iter(chunks);
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "echo",
                "description": "echo back the message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let msg = args["message"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: format!("echo: {msg}"),
            raw: Some(json!({ "echo": msg })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Builder forwarding tests (without skill feature)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_builder_forwarding_text_reply() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("Hello, world!".to_string()),
        StreamChunk::Stop,
    ]]));

    let runtime = AgentBuilder::new(llm.clone())
        .system_prompt("You are a helpful assistant")
        .build()
        .unwrap();

    let session_id = runtime.create_session().await;
    let result = runtime.run_turn_collect(session_id.clone(), "Hi").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");
    let (_events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(llm.call_count(), 1);
}

#[tokio::test]
async fn test_builder_forwarding_with_tool() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\": \"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![StreamChunk::Text("Done!".to_string()), StreamChunk::Stop],
    ]));

    let runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .build()
        .unwrap();

    let session_id = runtime.create_session().await;
    let result = runtime.run_turn_collect(session_id, "Echo hello").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");
    assert_eq!(llm.call_count(), 2);
}

#[tokio::test]
async fn test_builder_forwarding_middleware() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let triggered = Arc::new(AtomicBool::new(false));

    struct FlagMiddleware {
        flag: Arc<AtomicBool>,
    }

    #[async_trait]
    impl agent_base::Middleware for FlagMiddleware {
        async fn on_post_llm(&self, _ctx: &mut agent_base::PostLlmCtx) -> AgentResult<()> {
            self.flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("reply".to_string()),
        StreamChunk::Stop,
    ]]));

    let runtime = AgentBuilder::new(llm)
        .system_prompt("sys")
        .middleware(FlagMiddleware {
            flag: triggered.clone(),
        })
        .build()
        .unwrap();

    let session_id = runtime.create_session().await;
    let result = runtime.run_turn_collect(session_id, "test").await;
    assert!(result.is_ok());
    assert!(
        triggered.load(Ordering::SeqCst),
        "Middleware should be triggered"
    );
}

// ---------------------------------------------------------------------------
// Builder forwarding - error recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_builder_forwarding_error_recovery() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::ToolCall(json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_1",
                    "function": {
                        "name": "echo",
                        "arguments": "{\"message\": \"test\"}"
                    }
                }]
            }
        })),
        StreamChunk::Stop,
    ]]));

    let runtime = AgentBuilder::new(llm)
        .register_tool(EchoTool)
        .tool_timeout(30_000)
        .max_tool_output_chars(4096)
        .language(agent_base::Language::Zh)
        .build()
        .unwrap();

    let session_id = runtime.create_session().await;
    let result = runtime.run_turn_collect(session_id, "test").await;
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// ToolEnforcementMiddleware tests (in agent-base, re-exported by agent-works)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_enforcement_available_via_works() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("I will do it...".to_string()),
        StreamChunk::Stop,
    ]]));

    let config = agent_base::ToolEnforcementConfig::default();
    let runtime = AgentBuilder::new(llm)
        .register_tool(EchoTool)
        .system_prompt("sys")
        .middleware(agent_base::ToolEnforcementMiddleware::new(config))
        .build()
        .unwrap();

    let session_id = runtime.create_session().await;
    let result = runtime.run_turn_collect(session_id, "do something").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");
}

// ---------------------------------------------------------------------------
// Skill feature tests
// ---------------------------------------------------------------------------

#[cfg(feature = "skill")]
mod skill_tests {
    use super::*;
    use agent_base::RuntimeEvent;
    use agent_works::skill::{LazySkillPrompter, Skill, SkillPrompter};
    use serde_json::Value;
    use std::sync::Arc;

    struct AddTool;

    #[async_trait]
    impl Tool for AddTool {
        fn name(&self) -> &'static str {
            "add"
        }

        fn definition(&self) -> Value {
            json!({
                "type": "function",
                "function": {
                    "name": "add",
                    "description": "Calculate the sum of two integers",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "a": { "type": "integer", "description": "First addend" },
                            "b": { "type": "integer", "description": "Second addend" }
                        },
                        "required": ["a", "b"]
                    }
                }
            })
        }

        async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            Ok(ToolOutput {
                summary: format!("{a} + {b} = {}", a + b),
                raw: Some(json!({ "result": a + b })),
                control_flow: ToolControlFlow::Break,
                truncation: None,
            })
        }
    }

    struct MathSkill;

    impl Skill for MathSkill {
        fn name(&self) -> &'static str {
            "math"
        }

        fn brief_description(&self) -> String {
            "Math: supports addition".to_string()
        }

        fn detailed_description(&self) -> String {
            "## Math Skill\n\n- **add**: Calculate the sum of two integers".to_string()
        }

        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![Arc::new(AddTool)]
        }
    }

    #[tokio::test]
    async fn test_register_skill_with_builder() {
        let llm = Arc::new(super::MockLlmClient::new(vec![vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "add",
                            "arguments": "{\"a\": 1, \"b\": 2}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ]]));

        let runtime = AgentBuilder::new(llm)
            .system_prompt("You are a math assistant")
            .register_skill(MathSkill)
            .build()
            .unwrap();

        let session_id = runtime.create_session().await;
        let result = runtime.run_turn_collect(session_id, "1+2=?").await;
        assert!(result.is_ok(), "Expected ok, got: {result:?}");

        let (events, _outcome) = result.unwrap();
        let tool_done = events.iter().any(
            |e| matches!(e, RuntimeEvent::ToolCallFinished { tool_name, .. } if tool_name == "add"),
        );
        assert!(tool_done, "add tool should be called");
    }

    #[tokio::test]
    async fn test_skill_disable_prompt_injection() {
        let llm = Arc::new(super::MockLlmClient::new(vec![vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "add",
                            "arguments": "{\"a\": 3, \"b\": 4}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ]]));

        let runtime = AgentBuilder::new(llm)
            .system_prompt("My custom prompt")
            .register_skill(MathSkill)
            .disable_skill_prompt_injection()
            .build()
            .unwrap();

        let session_id = runtime.create_session().await;
        let result = runtime.run_turn_collect(session_id, "3+4=?").await;
        assert!(result.is_ok(), "Expected ok, got: {result:?}");
    }

    #[tokio::test]
    async fn test_skill_custom_detail_tool_name() {
        let llm = Arc::new(super::MockLlmClient::new(vec![vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "skill_info",
                            "arguments": "{\"name\": \"math\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ]]));

        let runtime = AgentBuilder::new(llm)
            .system_prompt("sys")
            .register_skill(MathSkill)
            .skill_detail_tool_name("skill_info")
            .build()
            .unwrap();

        let session_id = runtime.create_session().await;
        let result = runtime
            .run_turn_collect(session_id, "tell me about math skill")
            .await;
        assert!(result.is_ok(), "Expected ok, got: {result:?}");

        let (events, _outcome) = result.unwrap();
        let skill_loaded = events.iter().any(|e| {
            matches!(e, RuntimeEvent::ToolCallFinished { tool_name, summary, .. }
                if tool_name == "skill_info" && summary.contains("Math Skill"))
        });
        assert!(
            skill_loaded,
            "skill_info tool should return Math Skill detail"
        );
    }

    #[tokio::test]
    async fn test_skill_tool_name_conflict() {
        let llm = Arc::new(super::MockLlmClient::new(vec![]));

        let result = AgentBuilder::new(llm)
            .register_tool(AddTool) // registers "add" directly
            .register_skill(MathSkill) // MathSkill also registers "add"
            .build();

        assert!(result.is_err(), "Tool name conflict should be detected");
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(
            err_msg.contains("Tool name conflict"),
            "Error should mention tool name conflict: {err_msg}"
        );
    }

    #[test]
    fn test_lazy_skill_prompter() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(MathSkill)];
        let prompter = LazySkillPrompter::new();
        let prompt = prompter.build_prompt(&skills, "get_skill_detail");
        assert!(prompt.contains("math"), "Prompt should contain skill name");
        assert!(
            prompt.contains("get_skill_detail"),
            "Prompt should contain instruction"
        );
    }

    #[test]
    fn test_lazy_skill_prompter_custom_config() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(MathSkill)];
        let prompter = LazySkillPrompter::new()
            .title("## My Skills")
            .instruction("> Use {tool} to see details")
            .item_prefix("+ ");
        let prompt = prompter.build_prompt(&skills, "my_get_detail");
        assert!(prompt.contains("## My Skills"));
        assert!(prompt.contains("my_get_detail"));
        assert!(prompt.contains("+ "));
    }

    #[test]
    fn test_full_detail_prompter() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(MathSkill)];
        let prompter = agent_works::skill::FullDetailPrompter;
        let prompt = prompter.build_prompt(&skills, "get_skill_detail");
        assert!(prompt.contains("math"), "Should contain skill name");
        assert!(
            prompt.contains("Math Skill"),
            "Should contain detailed description"
        );
    }

    #[test]
    fn test_skill_default_methods() {
        assert_eq!(MathSkill.version(), "0.1.0");
        assert!(MathSkill.tags().is_empty());
        assert_eq!(MathSkill.author(), "");
    }
}

// ---------------------------------------------------------------------------
// Builtin tools tests
// ---------------------------------------------------------------------------

#[cfg(feature = "builtin-tools")]
mod builtin_tests {
    use super::*;
    use agent_works::builtin::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_read_file_builtin() {
        let tool = ReadFileTool {
            workspace: PathBuf::from("."),
        };
        assert_eq!(tool.name(), "read_file");

        let def = tool.definition();
        let func = def.get("function").unwrap();
        assert_eq!(func.get("name").unwrap().as_str().unwrap(), "read_file");
    }

    #[tokio::test]
    async fn test_write_file_builtin() {
        let tool = WriteFileTool {
            workspace: PathBuf::from("."),
        };
        assert_eq!(tool.name(), "write_file");
    }

    #[tokio::test]
    async fn test_list_directory_builtin() {
        let tool = ListDirectoryTool {
            workspace: PathBuf::from("."),
        };
        assert_eq!(tool.name(), "list_directory");
    }

    #[tokio::test]
    async fn test_file_exists_builtin() {
        let tool = FileExistsTool {
            workspace: PathBuf::from("."),
        };
        assert_eq!(tool.name(), "file_exists");
    }

    #[tokio::test]
    async fn test_search_replace_builtin() {
        let tool = SearchReplaceTool {
            workspace: PathBuf::from("."),
        };
        assert_eq!(tool.name(), "search_replace");
    }
}

// ---------------------------------------------------------------------------
// MCP module tests
// ---------------------------------------------------------------------------

#[cfg(feature = "mcp")]
mod mcp_tests {
    use super::*;
    use agent_works::mcp::*;

    #[test]
    fn test_mcp_tool_info_creation() {
        let info = McpToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({"type": "object"}),
        };
        assert_eq!(info.name, "test_tool");
        assert_eq!(info.description, "A test tool");
    }

    #[test]
    fn test_mcp_transport_variants() {
        let http = McpTransport::Http {
            url: "http://localhost:8080".to_string(),
        };
        assert!(matches!(http, McpTransport::Http { .. }));

        let stdio = McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "mcp-server".to_string()],
        };
        assert!(matches!(stdio, McpTransport::Stdio { .. }));
    }

    #[test]
    fn test_mcp_server_config() {
        let config = McpServerConfig {
            name: "my-server".to_string(),
            transport: McpTransport::Http {
                url: "http://localhost:8080".to_string(),
            },
            auto_reconnect: true,
        };
        assert_eq!(config.name, "my-server");
        assert!(config.auto_reconnect);
    }
}

// ---------------------------------------------------------------------------
// Skill detail tool standalone tests
// ---------------------------------------------------------------------------

#[cfg(feature = "skill")]
#[tokio::test]
async fn test_skill_detail_tool_standalone() {
    use agent_works::skill::{Skill, SkillDetailTool};
    use std::sync::Arc;

    struct SimpleSkill;
    impl Skill for SimpleSkill {
        fn name(&self) -> &'static str {
            "simple"
        }
        fn brief_description(&self) -> String {
            "A simple skill".to_string()
        }
        fn detailed_description(&self) -> String {
            "Detailed info about simple skill".to_string()
        }
        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![]
        }
    }

    let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(SimpleSkill)];
    let detail_tool = SkillDetailTool::new(skills, "get_skill_detail".to_string());

    assert_eq!(detail_tool.name(), "get_skill_detail");

    let def = detail_tool.definition();
    let func = def.get("function").unwrap();
    assert_eq!(
        func.get("name").unwrap().as_str().unwrap(),
        "get_skill_detail"
    );
}

// ---------------------------------------------------------------------------
// Path traversal protection tests (builtin-tools)
// ---------------------------------------------------------------------------

#[cfg(feature = "builtin-tools")]
mod path_traversal_tests {
    use agent_base::{Language, SessionId, Tool, ToolContext, UserEvent};
    use agent_works::builtin::*;
    use serde_json::json;

    fn make_ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<UserEvent>();
        ToolContext {
            session_id: SessionId::new(1),
            user_event_tx: tx,
            llm_client: None,
            session_store: None,
            language: Language::En,
            cancel_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Set up a real temp workspace with a test file in it.
    fn setup_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("secret.txt"), "top secret").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/nested.txt"), "nested content").unwrap();
        dir
    }

    // -- read_file traversal --

    #[tokio::test]
    async fn test_read_file_blocks_traversal() {
        let ws = setup_workspace();
        let tool = ReadFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "../../etc/passwd"}), &ctx).await;
        assert!(result.is_err(), "read_file should reject traversal path");
        // The error may say "outside workspace" or "failed to resolve parent directory"
        // depending on whether the target path exists. Either way, the traversal is blocked.
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("outside workspace") || err.contains("failed to resolve"),
            "error should block traversal: {err}"
        );
    }

    #[tokio::test]
    async fn test_read_file_blocks_null_byte() {
        let ws = setup_workspace();
        let tool = ReadFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "hello\0world"}), &ctx).await;
        assert!(result.is_err(), "read_file should reject null byte");
    }

    #[tokio::test]
    async fn test_read_file_allows_valid_path() {
        let ws = setup_workspace();
        let tool = ReadFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "secret.txt"}), &ctx).await;
        assert!(
            result.is_ok(),
            "read_file should allow valid path: {:?}",
            result.err()
        );
        assert!(result.unwrap().summary.contains("top secret"));
    }

    #[tokio::test]
    async fn test_read_file_allows_subdir_path() {
        let ws = setup_workspace();
        let tool = ReadFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "sub/nested.txt"}), &ctx).await;
        assert!(
            result.is_ok(),
            "read_file should allow subdir path: {:?}",
            result.err()
        );
        assert!(result.unwrap().summary.contains("nested content"));
    }

    // -- write_file traversal --

    #[tokio::test]
    async fn test_write_file_blocks_traversal() {
        let ws = setup_workspace();
        let tool = WriteFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool
            .call(
                &json!({"path": "../escape.txt", "content": "escaped"}),
                &ctx,
            )
            .await;
        assert!(result.is_err(), "write_file should reject traversal path");
    }

    // -- list_directory traversal --

    #[tokio::test]
    async fn test_list_directory_blocks_traversal() {
        let ws = setup_workspace();
        let tool = ListDirectoryTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "../.."}), &ctx).await;
        assert!(
            result.is_err(),
            "list_directory should reject traversal path"
        );
    }

    // -- file_exists traversal --

    #[tokio::test]
    async fn test_file_exists_blocks_traversal() {
        let ws = setup_workspace();
        let tool = FileExistsTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(&json!({"path": "../../etc/passwd"}), &ctx).await;
        assert!(result.is_err(), "file_exists should reject traversal path");
    }

    // -- search_replace traversal --

    #[tokio::test]
    async fn test_search_replace_blocks_traversal() {
        let ws = setup_workspace();
        let tool = SearchReplaceTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool
            .call(
                &json!({"path": "../../etc/passwd", "old_str": "root", "new_str": "hacked"}),
                &ctx,
            )
            .await;
        assert!(
            result.is_err(),
            "search_replace should reject traversal path"
        );
    }

    // -- search_replace old == new --

    #[tokio::test]
    async fn test_search_replace_same_old_new() {
        let ws = setup_workspace();
        let tool = SearchReplaceTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool
            .call(
                &json!({"path": "secret.txt", "old_str": "same", "new_str": "same"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.summary.contains("No changes needed"),
            "should short-circuit when old==new: {}",
            output.summary
        );
    }

    // -- search_replace text not found --

    #[tokio::test]
    async fn test_search_replace_not_found() {
        let ws = setup_workspace();
        let tool = SearchReplaceTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool.call(
            &json!({"path": "secret.txt", "old_str": "DOES_NOT_EXIST", "new_str": "replacement"}),
            &ctx,
        ).await;
        assert!(result.is_ok());
        assert!(result.unwrap().summary.contains("not found"));
    }

    // -- search_replace successful replacement --

    #[tokio::test]
    async fn test_search_replace_success() {
        let ws = setup_workspace();
        let tool = SearchReplaceTool {
            workspace: ws.path().to_path_buf(),
        };
        let ctx = make_ctx();

        let result = tool
            .call(
                &json!({"path": "secret.txt", "old_str": "top secret", "new_str": "declassified"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().summary.contains("Successfully"));

        // Verify the replacement
        let read_tool = ReadFileTool {
            workspace: ws.path().to_path_buf(),
        };
        let content = read_tool
            .call(&json!({"path": "secret.txt"}), &ctx)
            .await
            .unwrap();
        assert!(content.summary.contains("declassified"));
        assert!(!content.summary.contains("top secret"));
    }
}

// ---------------------------------------------------------------------------
// LazySkillPrompter dynamic tool name tests
// ---------------------------------------------------------------------------

#[cfg(feature = "skill")]
mod prompter_tests {
    use agent_base::Tool;
    use agent_works::skill::{FullDetailPrompter, LazySkillPrompter, Skill, SkillPrompter};
    use std::sync::Arc;

    struct TestSkill;
    impl Skill for TestSkill {
        fn name(&self) -> &'static str {
            "test_skill"
        }
        fn brief_description(&self) -> String {
            "A test skill".to_string()
        }
        fn detailed_description(&self) -> String {
            "Detailed test instructions".to_string()
        }
        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![]
        }
    }

    #[test]
    fn test_lazy_prompter_default_tool_name() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(TestSkill)];
        let prompter = LazySkillPrompter::new();
        let prompt = prompter.build_prompt(&skills, "get_skill_detail");
        assert!(
            prompt.contains("get_skill_detail"),
            "default prompt should mention the tool name"
        );
        assert!(prompt.contains("test_skill"));
    }

    #[test]
    fn test_lazy_prompter_custom_tool_name() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(TestSkill)];
        let prompter = LazySkillPrompter::new();
        let prompt = prompter.build_prompt(&skills, "my_custom_detail");
        assert!(
            prompt.contains("my_custom_detail"),
            "should use custom tool name"
        );
        assert!(
            !prompt.contains("get_skill_detail"),
            "should NOT contain default name"
        );
    }

    #[test]
    fn test_lazy_prompter_custom_instruction_with_placeholder() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(TestSkill)];
        let prompter =
            LazySkillPrompter::new().instruction("Use `{tool}` to learn more about skills.");
        let prompt = prompter.build_prompt(&skills, "detail_query");
        assert!(
            prompt.contains("Use `detail_query` to learn more"),
            "placeholder should be replaced: {}",
            prompt
        );
    }

    #[test]
    fn test_full_detail_prompter_ignores_tool_name() {
        let skills: Vec<Arc<dyn Skill>> = vec![Arc::new(TestSkill)];
        let prompter = FullDetailPrompter;
        let prompt = prompter.build_prompt(&skills, "whatever");
        assert!(prompt.contains("Detailed test instructions"));
        assert!(
            !prompt.contains("whatever"),
            "FullDetailPrompter should not use tool name"
        );
    }

    #[test]
    fn test_lazy_prompter_empty_skills() {
        let skills: Vec<Arc<dyn Skill>> = vec![];
        let prompter = LazySkillPrompter::new();
        let prompt = prompter.build_prompt(&skills, "get_skill_detail");
        assert!(
            prompt.is_empty(),
            "empty skills should produce empty prompt"
        );
    }
}

// ─────────────────────────────────────────────────────────────
// AgentHandle tests (十七、会话调度测试)
// ─────────────────────────────────────────────────────────────

mod agent_handle_tests {
    use super::*;
    use agent_base::RuntimeEvent;
    use agent_works::AgentHandle;

    /// 场景1：基本会话调度 — send_input → recv_event → RunFinished
    #[tokio::test]
    async fn test_handle_basic_session() {
        let llm = Arc::new(MockLlmClient::new(vec![vec![
            StreamChunk::Text("disk: 37% used".to_string()),
            StreamChunk::Stop,
        ]]));

        let runtime = AgentBuilder::new(llm)
            .system_prompt("You are a helpful assistant")
            .build()
            .unwrap();

        let mut handle = AgentHandle::new(runtime);
        handle.send_input("check disk space").await.unwrap();

        let mut all_events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.recv_event()).await {
            all_events.push(format!("{:?}", event));
            if matches!(event, RuntimeEvent::RunFinished { .. }) {
                break;
            }
        }

        assert!(
            all_events.iter().any(|e| e.contains("TextDelta")),
            "should have received TextDelta, got: {:?}",
            all_events
        );
        assert!(
            all_events.iter().any(|e| e.contains("RunFinished")),
            "should have received RunFinished, got: {:?}",
            all_events
        );
    }

    /// 场景2：取消会话 — send_input → cancel → RunCancelled
    #[tokio::test]
    async fn test_handle_cancel() {
        // Create a mock that uses a channel-based stream that blocks
        struct BlockingLlm {
            tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<AgentResult<StreamChunk>>>>>,
        }
        #[async_trait]
        impl LlmClient for BlockingLlm {
            async fn chat(
                &self,
                _: &[ChatMessage],
                _: &[Value],
                _: Option<&agent_base::ReasoningConfig>,
                _: Option<&ResponseFormat>,
            ) -> AgentResult<Value> {
                unimplemented!()
            }
            async fn chat_stream(
                &self,
                _: &[ChatMessage],
                _: &[Value],
                _: Option<&agent_base::ReasoningConfig>,
                _: Option<&ResponseFormat>,
            ) -> AgentResult<ChunkStream> {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AgentResult<StreamChunk>>();
                *self.tx.lock().unwrap() = Some(tx);
                // Create a stream from the channel receiver
                let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                    let item = rx.recv().await?;
                    Some((item, rx))
                });
                Ok(Box::pin(stream))
            }
            fn capabilities(&self) -> LlmCapabilities {
                LlmCapabilities::default()
            }
        }

        let llm = Arc::new(BlockingLlm {
            tx: Arc::new(Mutex::new(None)),
        });
        let llm_ref = llm.clone();

        let runtime = AgentBuilder::new(llm as Arc<dyn LlmClient>)
            .system_prompt("You are a helpful assistant")
            .build()
            .unwrap();

        let mut handle = AgentHandle::new(runtime);
        handle.send_input("long running task").await.unwrap();

        // Wait for the stream to be created (the LLM call happens)
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Send a chunk to start processing
        if let Some(tx) = llm_ref.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Ok(StreamChunk::Text("processing...".to_string())));
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel while stream is blocked waiting for more chunks
        handle.cancel();

        // Should receive RunCancelled
        let mut got_cancelled = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.recv_event()).await {
            if matches!(event, RuntimeEvent::RunCancelled { .. }) {
                got_cancelled = true;
                break;
            }
        }

        assert!(
            got_cancelled,
            "should have received RunCancelled after cancel()"
        );
    }

    /// 场景3：错误处理 — error → RunFinished (not hang)
    #[tokio::test]
    async fn test_handle_error_recovery() {
        // Use an LLM that returns an error
        struct ErrorLlm;
        #[async_trait]
        impl LlmClient for ErrorLlm {
            async fn chat(
                &self,
                _: &[ChatMessage],
                _: &[Value],
                _: Option<&agent_base::ReasoningConfig>,
                _: Option<&ResponseFormat>,
            ) -> AgentResult<Value> {
                Err(agent_base::AgentError::internal("simulated LLM failure"))
            }
            async fn chat_stream(
                &self,
                _: &[ChatMessage],
                _: &[Value],
                _: Option<&agent_base::ReasoningConfig>,
                _: Option<&ResponseFormat>,
            ) -> AgentResult<ChunkStream> {
                Err(agent_base::AgentError::internal("simulated LLM failure"))
            }
            fn capabilities(&self) -> LlmCapabilities {
                LlmCapabilities::default()
            }
        }

        let runtime = AgentBuilder::new(Arc::new(ErrorLlm))
            .system_prompt("test")
            .build()
            .unwrap();

        let mut handle = AgentHandle::new(runtime);
        handle.send_input("trigger error").await.unwrap();

        // Should receive RunFinished (not hang forever)
        let mut got_finished = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.recv_event()).await {
            if matches!(event, RuntimeEvent::RunFinished { .. }) {
                got_finished = true;
                break;
            }
        }

        assert!(
            got_finished,
            "should have received RunFinished after error, not hang"
        );
    }

    /// 场景2b：取消后可以继续发送新命令
    #[tokio::test]
    async fn test_handle_cancel_then_continue() {
        let llm = Arc::new(MockLlmClient::new(vec![
            // First response: never stops (will be cancelled)
            vec![StreamChunk::Text("processing...".to_string())],
            // Second response: normal
            vec![StreamChunk::Text("done!".to_string()), StreamChunk::Stop],
        ]));

        let runtime = AgentBuilder::new(llm)
            .system_prompt("test")
            .build()
            .unwrap();

        let mut handle = AgentHandle::new(runtime);

        // First turn: send + cancel
        handle.send_input("long task").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.cancel();

        // Drain events until RunCancelled
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.recv_event()).await {
            if matches!(event, RuntimeEvent::RunCancelled { .. }) {
                break;
            }
        }

        // Second turn: should work normally
        handle.send_input("check disk").await.unwrap();
        let mut got_finished = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, handle.recv_event()).await {
            if matches!(event, RuntimeEvent::RunFinished { .. }) {
                got_finished = true;
                break;
            }
        }

        assert!(got_finished, "should be able to continue after cancel");
    }
}
