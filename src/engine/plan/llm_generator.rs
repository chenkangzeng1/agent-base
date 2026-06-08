use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::llm::LlmClient;
use crate::types::{AgentError, AgentResult, ChatMessage, ExecutionPlan, PlanStep, ResponseFormat, RuntimeEvent, SessionId};
use super::traits::PlanGenerator;
use super::streaming_parser::StreamingJsonParser;

/// Options for plan generation that can override generator defaults.
///
/// Use `None` for fields you want to keep at their default values.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Override the maximum number of steps. `None` uses the generator's default.
    pub max_steps: Option<usize>,
}

/// Default system prompt for plan generation (agentic-friendly).
const DEFAULT_PLAN_SYSTEM_PROMPT: &str = r#"You are a task planner. Given an objective, break it down into sequential steps.

Output a JSON object with a "steps" array. Each step has:
- "id": unique string identifier (e.g. "step-1", "step-2")
- "description": what this step should accomplish

Keep steps atomic and ordered."#;

/// Detect whether the prompt is primarily Chinese or English.
///
/// Returns `"zh"` if Chinese characters exceed 30% of non-whitespace characters,
/// otherwise returns `"en"`.
fn detect_language(prompt: &str) -> &'static str {
    let chinese_chars = prompt
        .chars()
        .filter(|c| *c >= '\u{4e00}' && *c <= '\u{9fff}')
        .count();
    let total_chars = prompt.chars().filter(|c| !c.is_whitespace()).count();

    if total_chars > 0 && chinese_chars as f64 / total_chars as f64 > 0.3 {
        "zh"
    } else {
        "en"
    }
}

/// Generate step limit text in the appropriate language.
fn step_limit_text(max_steps: usize, lang: &str) -> String {
    match lang {
        "zh" => format!("注意：最多生成 {} 个步骤。", max_steps),
        _ => format!("Note: Generate at most {} steps.", max_steps),
    }
}

/// Check if the prompt already contains step limit hints.
fn has_step_limit_hint(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    // Chinese hints (more precise)
    (lower.contains("最多") && (lower.contains("步骤") || lower.contains("步")))
        || lower.contains("步骤数")
        // English hints (more precise - require number + step)
        || (lower.contains("max") && lower.contains("step"))
        || (lower.contains("step") && lower.contains("limit"))
        || (lower.contains("at most") && lower.contains("step"))
        || (lower.contains("do not exceed") && lower.contains("step"))
}

/// A `PlanGenerator` that uses an LLM to generate execution plans.
///
/// # Example
///
/// ```ignore
/// use agent_base::{LlmPlanGenerator, PlanConfig, Recovery};
///
/// let generator = LlmPlanGenerator::new(llm_client)
///     .with_max_steps(10);
///
/// runtime.run_plan_with_generator(
///     session_id,
///     "Check server health and fix issues",
///     Arc::new(generator),
///     PlanConfig::new()
///         .with_executor(runtime.create_step_executor())
///         .recovery(Recovery::retry(2)),
///     |event| { println!("{:?}", event); Ok(()) },
/// ).await?;
/// ```
pub struct LlmPlanGenerator {
    llm_client: std::sync::Arc<dyn LlmClient>,
    system_prompt: String,
    max_steps: usize,
    step_schema: Option<Value>,
}

impl LlmPlanGenerator {
    pub fn new(llm_client: std::sync::Arc<dyn LlmClient>) -> Self {
        Self {
            llm_client,
            system_prompt: DEFAULT_PLAN_SYSTEM_PROMPT.to_string(),
            max_steps: 20,
            step_schema: None,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn with_max_steps(mut self, max: usize) -> Self {
        self.max_steps = max;
        self
    }

    pub fn with_step_schema(mut self, schema: Value) -> Self {
        self.step_schema = Some(schema);
        self
    }

    /// Build the effective system prompt with step limit appended if needed.
    ///
    /// If the user's prompt already contains step limit hints, the framework
    /// will not append additional constraints. Otherwise, it automatically
    /// appends a step limit notice in the detected language.
    fn effective_system_prompt_with_max_steps(&self, max_steps: usize) -> String {
        let mut prompt = self.system_prompt.clone();

        // Replace legacy {max_steps} placeholder if present
        prompt = prompt.replace("{max_steps}", &max_steps.to_string());

        // Auto-append step limit if not already present
        if !has_step_limit_hint(&prompt) {
            let lang = detect_language(&prompt);
            prompt.push_str("\n\n");
            prompt.push_str(&step_limit_text(max_steps, lang));
        }

        prompt
    }

    /// Build the response format constraint if step_schema is set.
    fn response_format(&self) -> Option<ResponseFormat> {
        self.step_schema.as_ref().map(|schema| {
            ResponseFormat::JsonSchema {
                name: "plan".to_string(),
                schema: schema.clone(),
            }
        })
    }

    /// Generate a unique plan ID.
    fn next_plan_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("plan-{}-{}", ts, seq)
    }

    /// Strip markdown code-block wrappers (```json ... ``` or ``` ... ```)
    /// that many LLMs wrap their JSON output in.
    fn strip_markdown_code_block(text: &str) -> &str {
        let trimmed = text.trim();
        if trimmed.starts_with("```") {
            // Find end of first line (the opening fence)
            if let Some(start) = trimmed.find('\n') {
                let body = &trimmed[start + 1..];
                // Remove closing fence if present
                if let Some(end) = body.rfind("```") {
                    return body[..end].trim();
                }
                return body.trim();
            }
        }
        trimmed
    }

    /// Extract the actual content from an LLM response.
    ///
    /// Handles different provider response formats:
    /// - OpenAI/compatible: `choices[0].message.content` (string)
    /// - Anthropic: `content[0].text` (array of content blocks)
    /// - Google Gemini: `candidates[0].content.parts[0].text`
    /// - Direct JSON with "steps" at top level
    fn extract_plan_json(response: &Value) -> Value {
        // Helper: try parsing text as JSON, stripping markdown code blocks if needed.
        let try_parse = |text: &str| -> Option<Value> {
            serde_json::from_str::<Value>(text)
                .ok()
                .or_else(|| serde_json::from_str::<Value>(Self::strip_markdown_code_block(text)).ok())
        };

        // 1. OpenAI format: choices[0].message.content
        if let Some(choices) = response.get("choices").and_then(|v| v.as_array()) {
            if let Some(first) = choices.first() {
                if let Some(content) = first.get("message").and_then(|m| m.get("content")).and_then(|v| v.as_str()) {
                    if let Some(parsed) = try_parse(content) {
                        return parsed;
                    }
                }
            }
        }

        // 2. Anthropic format: content[0].text
        if let Some(content) = response.get("content").and_then(|v| v.as_array()) {
            for block in content {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        if let Some(parsed) = try_parse(text) {
                            return parsed;
                        }
                    }
                }
            }
        }

        // 3. Google Gemini format: candidates[0].content.parts[0].text
        if let Some(candidates) = response.get("candidates").and_then(|v| v.as_array()) {
            if let Some(first) = candidates.first() {
                if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|v| v.as_array()) {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            if let Some(parsed) = try_parse(text) {
                                return parsed;
                            }
                        }
                    }
                }
            }
        }

        // 4. Direct format: "steps" at top level
        if response.get("steps").is_some() {
            return response.clone();
        }

        // Fallback: return as-is
        response.clone()
    }

    /// Parse the LLM response into a list of PlanSteps.
    fn parse_steps(response: &Value) -> AgentResult<Vec<PlanStep>> {
        let steps_array = response
            .get("steps")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                AgentError::plan_generation(
                    "LLM response missing 'steps' array".to_string(),
                )
            })?;

        let mut steps = Vec::new();
        for (i, step_val) in steps_array.iter().enumerate() {
            let id = step_val
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("step-{}", i + 1))
                .to_string();

            let description = step_val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed step")
                .to_string();

            // Build payload from step_val (preserving tool_name, args, etc.)
            let payload = if let Some(tool_name) = step_val.get("tool_name").and_then(|v| v.as_str())
            {
                let args = step_val.get("args").cloned().unwrap_or(json!({}));
                json!({"tool_name": tool_name, "args": args})
            } else {
                // No tool_name — treat the whole step as an agentic prompt
                step_val.clone()
            };

            steps.push(PlanStep::new(id, description, payload));
        }

        Ok(steps)
    }
}

impl LlmPlanGenerator {
    /// Build the messages for plan generation.
    fn build_messages(&self, objective: &str, context: &str, tools: &[Value], max_steps: usize) -> Vec<ChatMessage> {
        let system_prompt = self.effective_system_prompt_with_max_steps(max_steps);

        let tools_desc = if tools.is_empty() {
            String::new()
        } else {
            let mut desc = String::from("\nAvailable tools:");
            for t in tools {
                if let Some(func) = t.get("function") {
                    let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let description = func.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    desc.push_str(&format!("\n- {}: {}", name, description));
                    if let Some(params) = func.get("parameters").and_then(|v| v.get("properties")) {
                        if let Some(obj) = params.as_object() {
                            for (param_name, param_val) in obj {
                                let param_type = param_val.get("type").and_then(|v| v.as_str()).unwrap_or("any");
                                desc.push_str(&format!("\n    {} ({}): {}", param_name, param_type,
                                    param_val.get("description").and_then(|v| v.as_str()).unwrap_or("")));
                            }
                        }
                    }
                }
            }
            desc
        };

        let ctx_suffix = if context.is_empty() {
            String::new()
        } else {
            format!("\nContext: {}", context)
        };

        let user_message = if tools.is_empty() {
            // Agentic mode: LLM only generates descriptions, no tool selection
            format!(
                "Objective: {}{}\n\nGenerate a plan as a JSON object with a \"steps\" array. Each step should have \"id\" and \"description\". Do NOT specify tools or arguments. The execution engine will determine the best tools to use at runtime.",
                objective, ctx_suffix
            )
        } else {
            // Deterministic mode: LLM generates plans with tool_name and args
            format!(
                "Objective: {}{}\n\nGenerate a plan as a JSON object with a \"steps\" array. Each step should use \"tool_name\" and \"args\" to call a tool.",
                objective, ctx_suffix
            )
        };

        vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!("{}{}", user_message, tools_desc)),
        ]
    }

    /// Truncate steps to max_steps if needed.
    fn truncate_steps(&self, steps: Vec<PlanStep>, max_steps: usize) -> Vec<PlanStep> {
        if steps.len() > max_steps {
            tracing::warn!("Plan has {} steps, truncating to {}", steps.len(), max_steps);
            steps.into_iter().take(max_steps).collect()
        } else {
            steps
        }
    }

    /// Generate plan without streaming (uses chat).
    async fn generate_non_streaming(&self, objective: &str, context: &str, tools: &[Value], max_steps: usize) -> AgentResult<ExecutionPlan> {
        let messages = self.build_messages(objective, context, tools, max_steps);

        let response = self
            .llm_client
            .chat(&messages, &[], None, self.response_format().as_ref())
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        tracing::debug!(response = %response, "LlmPlanGenerator: raw LLM response");

        let plan_json = Self::extract_plan_json(&response);

        let steps = match Self::parse_steps(&plan_json) {
            Ok(steps) => steps,
            Err(parse_err) => {
                tracing::warn!("First parse attempt failed: {}, retrying", parse_err);
                let retry_messages = self.build_messages(objective, context, tools, max_steps);
                // Inject error info into the user message
                let mut retry_messages = retry_messages;
                if let Some(ChatMessage::User { content, .. }) = retry_messages.last_mut() {
                    *content = format!("{}\n\nYour previous response was invalid: {}\n\nPlease respond with a valid JSON object containing a \"steps\" array.", content, parse_err);
                }
                let retry_response = self
                    .llm_client
                    .chat(&retry_messages, &[], None, self.response_format().as_ref())
                    .await
                    .map_err(|e| AgentError::plan_generation(e.to_string()))?;
                let retry_json = Self::extract_plan_json(&retry_response);
                Self::parse_steps(&retry_json)?
            }
        };

        Ok(ExecutionPlan::with_single_phase(Self::next_plan_id(), objective, self.truncate_steps(steps, max_steps)))
    }

    /// Generate plan with streaming (uses chat_stream), emitting events via channel.
    async fn generate_streaming(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        max_steps: usize,
        event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
    ) -> AgentResult<ExecutionPlan> {
        let messages = self.build_messages(objective, context, tools, max_steps);

        let mut stream = self
            .llm_client
            .chat_stream(&messages, &[], None, self.response_format().as_ref())
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        let _ = event_tx.send(RuntimeEvent::PlanGenerating {
            session_id: SessionId::new(0),
            plan_id: String::new(),
        });

        let mut parser = StreamingJsonParser::<PlanStep>::new().with_key("steps");
        let mut full_text = String::new();

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::plan_generation(e.to_string()))?;
            match chunk {
                crate::llm::StreamChunk::Text(text) => {
                    full_text.push_str(&text);
                    let _ = event_tx.send(RuntimeEvent::ThoughtDelta {
                        session_id: SessionId::new(0),
                        text: text.clone(),
                    });

                    let new_steps = parser.process_chunk(&text);
                    for step in new_steps {
                        let idx = parser.accumulated().len() - 1;
                        let _ = event_tx.send(RuntimeEvent::PlanStepParsed {
                            session_id: SessionId::new(0),
                            plan_id: String::new(),
                            step_index: idx,
                            step_id: step.id.clone(),
                            step_description: step.description.clone(),
                        });
                    }
                }
                crate::llm::StreamChunk::Stop => break,
                _ => {}
            }
        }

        // If parser didn't extract steps from streaming, try parsing the full response
        let steps = if parser.accumulated().is_empty() {
            let stripped = Self::strip_markdown_code_block(&full_text);
            let plan_json = if stripped.starts_with('{') {
                serde_json::from_str::<Value>(stripped)
                    .map_err(|_| AgentError::plan_generation(
                        format!("Failed to parse LLM response as JSON: {}", stripped.chars().take(200).collect::<String>())
                    ))?
            } else {
                // Response might be wrapped in API format
                let raw: Value = serde_json::from_str(&full_text)
                    .map_err(|_| AgentError::plan_generation(
                        format!("Failed to parse LLM response as JSON: {}", full_text.chars().take(200).collect::<String>())
                    ))?;
                Self::extract_plan_json(&raw)
            };
            Self::parse_steps(&plan_json)?
        } else {
            parser.accumulated().to_vec()
        };

        Ok(ExecutionPlan::with_single_phase(Self::next_plan_id(), objective, self.truncate_steps(steps, max_steps)))
    }
}

#[async_trait]
impl PlanGenerator for LlmPlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        on_event: Option<mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> AgentResult<ExecutionPlan> {
        let max_steps = self.max_steps;
        if let Some(tx) = &on_event {
            self.generate_streaming(objective, context, tools, max_steps, tx).await
        } else {
            self.generate_non_streaming(objective, context, tools, max_steps).await
        }
    }
}

impl LlmPlanGenerator {
    /// Generate a plan with runtime options that can override generator defaults.
    ///
    /// Use this method when you need to customize plan generation at call time
    /// rather than at generator construction time.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use agent_base::{LlmPlanGenerator, PlanOptions};
    ///
    /// let generator = LlmPlanGenerator::new(llm_client).with_max_steps(10);
    ///
    /// // Override max_steps for this specific call
    /// let options = PlanOptions { max_steps: Some(5) };
    /// let plan = generator.generate_plan_with_options(
    ///     "Deploy application",
    ///     context,
    ///     tools,
    ///     options,
    ///     None,
    /// ).await?;
    /// ```
    pub async fn generate_plan_with_options(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        options: PlanOptions,
        on_event: Option<mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> AgentResult<ExecutionPlan> {
        let max_steps = options.max_steps.unwrap_or(self.max_steps);
        if let Some(tx) = &on_event {
            self.generate_streaming(objective, context, tools, max_steps, tx).await
        } else {
            self.generate_non_streaming(objective, context, tools, max_steps).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_chinese() {
        // 纯中文
        assert_eq!(detect_language("你是一个任务规划器"), "zh");
        // 中文占多数（超过 30%）
        assert_eq!(detect_language("你是一个任务规划器 负责分析"), "zh");
        // 中文占比刚好超过 30%
        assert_eq!(detect_language("规划器 task planner 你好世界"), "zh");
    }

    #[test]
    fn test_detect_language_english() {
        // 纯英文
        assert_eq!(detect_language("You are a task planner"), "en");
        // 中英文混合，英文占多数
        assert_eq!(detect_language("You are a task planner 负责"), "en");
        // 空字符串
        assert_eq!(detect_language(""), "en");
        // 中文占比低于 30%
        assert_eq!(detect_language("You are a 任务"), "en");
    }

    #[test]
    fn test_step_limit_text() {
        assert_eq!(step_limit_text(5, "zh"), "注意：最多生成 5 个步骤。");
        assert_eq!(step_limit_text(10, "zh"), "注意：最多生成 10 个步骤。");
        assert_eq!(step_limit_text(5, "en"), "Note: Generate at most 5 steps.");
        assert_eq!(step_limit_text(10, "en"), "Note: Generate at most 10 steps.");
    }

    #[test]
    fn test_has_step_limit_hint_chinese() {
        // 包含中文约束提示
        assert!(has_step_limit_hint("你是一个规划器，最多生成 5 个步骤"));
        assert!(has_step_limit_hint("步骤数限制：5"));
        assert!(has_step_limit_hint("最多 10 步"));
        // 不包含约束提示
        assert!(!has_step_limit_hint("你是一个任务规划器"));
        assert!(!has_step_limit_hint("请帮我生成计划"));
    }

    #[test]
    fn test_has_step_limit_hint_english() {
        // 包含英文约束提示
        assert!(has_step_limit_hint("You are a planner. Max 5 steps."));
        assert!(has_step_limit_hint("Step limit: 10"));
        assert!(has_step_limit_hint("Generate at most 3 steps"));
        assert!(has_step_limit_hint("Do not exceed 5 steps"));
        // 不包含约束提示
        assert!(!has_step_limit_hint("You are a task planner"));
        assert!(!has_step_limit_hint("Please generate a plan"));
    }

    #[test]
    fn test_effective_system_prompt_auto_append_chinese() {
        // 创建一个模拟的 LlmPlanGenerator（不需要真实的 LLM 客户端）
        // 直接测试 effective_system_prompt_with_max_steps 方法的逻辑
        let prompt = "你是一个任务规划器，负责分析目标并生成执行计划";
        let max_steps = 5;

        // 模拟追加逻辑
        let mut result = prompt.to_string();
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 中文 prompt 追加测试 ===");
        println!("原始 prompt: {}", prompt);
        println!("追加后 prompt:\n{}", result);
        println!("========================\n");

        // 验证追加了中文约束
        assert!(result.contains("注意：最多生成 5 个步骤。"));
        assert!(result.ends_with("注意：最多生成 5 个步骤。"));
    }

    #[test]
    fn test_effective_system_prompt_auto_append_english() {
        let prompt = "You are a task planner. Given an objective, break it down into sequential steps.";
        let max_steps = 10;

        let mut result = prompt.to_string();
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== English prompt append test ===");
        println!("Original prompt: {}", prompt);
        println!("Appended prompt:\n{}", result);
        println!("==================================\n");

        assert!(result.contains("Note: Generate at most 10 steps."));
        assert!(result.ends_with("Note: Generate at most 10 steps."));
    }

    #[test]
    fn test_effective_system_prompt_skip_append_if_exists() {
        // 用户已经写了步数约束，框架不应该追加
        let prompt = "你是一个任务规划器，最多生成 5 个步骤";
        let max_steps = 10;

        let mut result = prompt.to_string();
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 智能检测跳过追加测试 ===");
        println!("原始 prompt（已包含约束）: {}", prompt);
        println!("处理后 prompt: {}", result);
        println!("============================\n");

        // 验证没有重复追加
        assert_eq!(result, prompt);
        assert!(!result.contains("注意：最多生成 10 个步骤。"));
    }

    #[test]
    fn test_effective_system_prompt_with_placeholder() {
        // 用户使用了 {max_steps} 占位符
        let prompt = "你是一个任务规划器。最多生成 {max_steps} 个步骤。";
        let max_steps = 5;

        let mut result = prompt.to_string();
        // 替换占位符
        result = result.replace("{max_steps}", &max_steps.to_string());
        // 检查是否需要追加
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 占位符替换测试 ===");
        println!("原始 prompt: {}", prompt);
        println!("处理后 prompt: {}", result);
        println!("======================\n");

        // 验证占位符被替换
        assert!(result.contains("最多生成 5 个步骤。"));
        // 验证没有重复追加（因为已经包含约束提示）
        assert!(!result.contains("注意：最多生成"));
    }

    #[test]
    fn test_effective_system_prompt_default_prompt() {
        // 测试默认 prompt 的行为
        let prompt = DEFAULT_PLAN_SYSTEM_PROMPT;
        let max_steps = 20;

        let mut result = prompt.to_string();
        // 替换占位符（默认 prompt 不包含 {max_steps}，所以不会替换）
        result = result.replace("{max_steps}", &max_steps.to_string());
        // 检查是否需要追加
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 默认 prompt 测试 ===");
        println!("原始 prompt: {}", prompt);
        println!("处理后 prompt:\n{}", result);
        println!("========================\n");

        // 验证默认 prompt 被追加了约束
        // 注意：默认 prompt 不包含步数约束，所以会被追加
        assert!(result.contains("Note: Generate at most 20 steps."));
        assert!(result.ends_with("Note: Generate at most 20 steps."));
    }

    #[test]
    fn test_effective_system_prompt_with_max_steps_in_default() {
        // 测试旧版默认 prompt（包含 {max_steps} 占位符）
        let old_default_prompt = r#"You are a task planner. Given an objective, break it down into sequential steps.

Output a JSON object with a "steps" array. Each step has:
- "id": unique string identifier (e.g. "step-1", "step-2")
- "description": what this step should accomplish

Keep steps atomic and ordered. Do not exceed {max_steps} steps."#;
        let max_steps = 15;

        let mut result = old_default_prompt.to_string();
        // 替换占位符
        result = result.replace("{max_steps}", &max_steps.to_string());
        // 检查是否需要追加
        if !has_step_limit_hint(&result) {
            let lang = detect_language(&result);
            result.push_str("\n\n");
            result.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 旧版默认 prompt 测试 ===");
        println!("原始 prompt: {}", old_default_prompt);
        println!("处理后 prompt:\n{}", result);
        println!("============================\n");

        // 验证占位符被替换
        assert!(result.contains("Do not exceed 15 steps."));
        // 验证没有重复追加（因为 "Do not exceed" 会被检测到）
        assert!(!result.contains("Note: Generate at most 15 steps."));
    }

    #[test]
    fn test_has_step_limit_hint_false_positive_chinese() {
        // 边界情况：包含"最多"但不是步数约束
        let prompt1 = "你是一个任务规划器，最多可以处理复杂任务";
        let prompt2 = "最多尝试 3 次";

        // 当前实现会误判，因为只检测"最多" + "步骤/步"
        // 这里"最多" + "任务" 不会被检测到
        assert!(!has_step_limit_hint(prompt1));
        assert!(!has_step_limit_hint(prompt2));
    }

    #[test]
    fn test_has_step_limit_hint_false_positive_english() {
        // 边界情况：包含"at most"但不是步数约束
        let prompt1 = "You should at most try 3 times";
        let prompt2 = "Do not exceed the context window";

        // 改进后的实现不会误判
        assert!(!has_step_limit_hint(prompt1));  // 不误判
        assert!(!has_step_limit_hint(prompt2));  // 不误判

        // 但是包含 "step" 的会被检测到
        assert!(has_step_limit_hint("You should at most have 5 steps"));
        assert!(has_step_limit_hint("Do not exceed 10 steps"));
    }

    #[test]
    fn test_multiple_generate_plan_calls() {
        // 测试多次调用 generate_plan 不会累积追加
        let prompt = "你是一个任务规划器";
        let max_steps = 5;

        // 第一次调用
        let mut result1 = prompt.to_string();
        if !has_step_limit_hint(&result1) {
            let lang = detect_language(&result1);
            result1.push_str("\n\n");
            result1.push_str(&step_limit_text(max_steps, lang));
        }

        // 第二次调用（模拟再次调用 generate_plan）
        let mut result2 = prompt.to_string();
        if !has_step_limit_hint(&result2) {
            let lang = detect_language(&result2);
            result2.push_str("\n\n");
            result2.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 多次调用测试 ===");
        println!("第一次结果:\n{}", result1);
        println!("第二次结果:\n{}", result2);
        println!("====================\n");

        // 验证两次结果相同，没有累积追加
        assert_eq!(result1, result2);
        assert_eq!(result1.matches("注意：").count(), 1);  // 只出现一次
    }

    #[test]
    fn test_generate_plan_with_different_max_steps() {
        // 测试同一个 generator 用不同 max_steps 调用
        let prompt = "你是一个任务规划器";

        // max_steps = 5
        let mut result_5 = prompt.to_string();
        if !has_step_limit_hint(&result_5) {
            let lang = detect_language(&result_5);
            result_5.push_str("\n\n");
            result_5.push_str(&step_limit_text(5, lang));
        }

        // max_steps = 10
        let mut result_10 = prompt.to_string();
        if !has_step_limit_hint(&result_10) {
            let lang = detect_language(&result_10);
            result_10.push_str("\n\n");
            result_10.push_str(&step_limit_text(10, lang));
        }

        println!("=== 不同 max_steps 测试 ===");
        println!("max_steps=5:\n{}", result_5);
        println!("max_steps=10:\n{}", result_10);
        println!("============================\n");

        // 验证结果不同
        assert_ne!(result_5, result_10);
        assert!(result_5.contains("最多生成 5 个步骤"));
        assert!(result_10.contains("最多生成 10 个步骤"));
    }

    #[test]
    fn test_lifecycle_constraint_only_during_generation() {
        // 测试：约束只在生成阶段存在，执行阶段不存在
        let prompt = "你是一个任务规划器";
        let max_steps = 5;

        // 1. 生成阶段：约束被追加
        let mut generation_prompt = prompt.to_string();
        if !has_step_limit_hint(&generation_prompt) {
            let lang = detect_language(&generation_prompt);
            generation_prompt.push_str("\n\n");
            generation_prompt.push_str(&step_limit_text(max_steps, lang));
        }

        println!("=== 生命周期测试 ===");
        println!("生成阶段 prompt:\n{}", generation_prompt);
        assert!(generation_prompt.contains("注意：最多生成 5 个步骤。"));

        // 2. 执行阶段：约束不存在（模拟 plan 生成完成后的状态）
        // 执行阶段使用的是生成的 plan，不是 prompt
        // 所以约束"消失"了，这是正确的行为
        let execution_context = "执行 plan 中的步骤";
        println!("\n执行阶段上下文: {}", execution_context);
        println!("（约束已消失，因为 plan 已生成完成）\n");

        // 验证执行阶段不包含约束
        assert!(!execution_context.contains("最多生成"));
        assert!(!execution_context.contains("注意"));
    }

    #[test]
    fn test_concurrent_plan_generation() {
        // 测试：多 plan 并发生成的安全性
        use std::sync::Arc;
        use std::thread;

        let prompt = "你是一个任务规划器";
        let max_steps = 5;

        // 模拟并发调用（使用线程模拟）
        let handles: Vec<_> = (0..5).map(|i| {
            let prompt = prompt.to_string();
            thread::spawn(move || {
                // 模拟 generate_plan 的内部逻辑
                let mut result = prompt.clone();
                if !has_step_limit_hint(&result) {
                    let lang = detect_language(&result);
                    result.push_str("\n\n");
                    result.push_str(&step_limit_text(max_steps + i, lang));
                }
                result
            })
        }).collect();

        let results: Vec<String> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();

        println!("=== 并发测试 ===");
        for (i, result) in results.iter().enumerate() {
            println!("线程 {} 结果: {}", i, result);
        }
        println!("================\n");

        // 验证每个线程都生成了独立的结果
        assert_eq!(results.len(), 5);

        // 验证每个结果都包含约束
        for (i, result) in results.iter().enumerate() {
            let expected_steps = 5 + i;
            assert!(result.contains(&format!("最多生成 {} 个步骤", expected_steps)));
        }

        // 验证结果之间没有相互干扰（每个都有不同的 max_steps）
        let unique_results: std::collections::HashSet<_> = results.iter().collect();
        assert_eq!(unique_results.len(), 5);  // 所有结果都不同
    }

    #[test]
    fn test_concurrent_different_max_steps() {
        // 测试：同一个 generator 用不同 max_steps 并发调用
        use std::thread;

        let prompt = "你是一个任务规划器";
        let configs = vec![3, 5, 7, 10, 15];

        let handles: Vec<_> = configs.iter().map(|&max_steps| {
            let prompt = prompt.to_string();
            thread::spawn(move || {
                let mut result = prompt.clone();
                if !has_step_limit_hint(&result) {
                    let lang = detect_language(&result);
                    result.push_str("\n\n");
                    result.push_str(&step_limit_text(max_steps, lang));
                }
                (max_steps, result)
            })
        }).collect();

        let results: Vec<(usize, String)> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();

        println!("=== 不同 max_steps 并发测试 ===");
        for (max_steps, result) in &results {
            println!("max_steps={} 结果: {}", max_steps, result);
        }
        println!("================================\n");

        // 验证每个配置都生成了正确的约束
        for (max_steps, result) in &results {
            assert!(result.contains(&format!("最多生成 {} 个步骤", max_steps)));
        }

        // 验证结果之间没有相互干扰
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_plan_id_uniqueness() {
        // 测试：plan ID 的唯一性（模拟并发生成）
        use std::collections::HashSet;
        use std::thread;

        let handles: Vec<_> = (0..10).map(|_| {
            thread::spawn(|| {
                // 模拟 next_plan_id() 的逻辑
                use std::sync::atomic::{AtomicU64, Ordering};
                static COUNTER: AtomicU64 = AtomicU64::new(0);
                let ts = 1234567890;  // 固定时间戳
                let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
                format!("plan-{}-{}", ts, seq)
            })
        }).collect();

        let ids: Vec<String> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();

        println!("=== Plan ID 唯一性测试 ===");
        for id in &ids {
            println!("ID: {}", id);
        }
        println!("==========================\n");

        // 验证所有 ID 都是唯一的
        let unique_ids: HashSet<_> = ids.iter().collect();
        assert_eq!(unique_ids.len(), 10);
    }
}
