use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::LlmClient;
use crate::types::{AgentError, AgentResult, ChatMessage, ExecutionPlan, PlanStep, ResponseFormat};
use super::traits::PlanGenerator;
use super::streaming_parser::StreamingJsonParser;

/// Default system prompt for plan generation.
const DEFAULT_PLAN_SYSTEM_PROMPT: &str = r#"You are a task planner. Given an objective and available tools, break it down into sequential steps.

Output a JSON object with a "steps" array. Each step has:
- "id": unique string identifier (e.g. "step-1", "step-2")
- "description": what this step does
- "tool_name": (optional) name of the tool to call
- "args": (optional) arguments for the tool

If a step has "tool_name", it will be executed as a direct tool call.
If a step does not have "tool_name", it will be executed as an LLM-driven agent turn.

Keep steps atomic and ordered. Do not exceed {max_steps} steps."#;

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
///         .executor(runtime.create_step_executor())
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

    /// Build the effective system prompt with `{max_steps}` replaced.
    fn effective_system_prompt(&self) -> String {
        self.system_prompt
            .replace("{max_steps}", &self.max_steps.to_string())
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

#[async_trait]
impl PlanGenerator for LlmPlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
    ) -> AgentResult<ExecutionPlan> {
        let system_prompt = self.effective_system_prompt();

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

        let user_message = format!(
            "Objective: {}{}\n\nGenerate a plan as a JSON object with a \"steps\" array. Each step should use \"tool_name\" and \"args\" to call a tool.",
            objective,
            if context.is_empty() {
                String::new()
            } else {
                format!("\nContext: {}", context)
            }
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!("{}{}", user_message, tools_desc)),
        ];

        let response = self
            .llm_client
            .chat(&messages, &[], None, self.response_format().as_ref())
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        // Try to parse the response. If it fails, retry once with the error message.
        let steps = match Self::parse_steps(&response) {
            Ok(steps) => steps,
            Err(parse_err) => {
                tracing::warn!("First parse attempt failed: {}, retrying", parse_err);

                let retry_messages = vec![
                    ChatMessage::system(self.effective_system_prompt()),
                    ChatMessage::user(format!(
                        "{}{}\n\nYour previous response was invalid: {}\n\nPlease respond with a valid JSON object containing a \"steps\" array.",
                        user_message, tools_desc, parse_err
                    )),
                ];

                let retry_response = self
                    .llm_client
                    .chat(&retry_messages, &[], None, self.response_format().as_ref())
                    .await
                    .map_err(|e| AgentError::plan_generation(e.to_string()))?;

                Self::parse_steps(&retry_response)?
            }
        };

        // Truncate if over max_steps
        let steps = if steps.len() > self.max_steps {
            tracing::warn!(
                "Plan has {} steps, truncating to {}",
                steps.len(),
                self.max_steps
            );
            steps.into_iter().take(self.max_steps).collect()
        } else {
            steps
        };

        Ok(ExecutionPlan::with_single_phase(Self::next_plan_id(), objective, steps))
    }

    async fn generate_plan_streaming(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        on_generating: Box<dyn Fn() + Send>,
        on_step_parsed: Box<dyn Fn(usize, String, String) + Send>,
        on_thought: Box<dyn Fn(String) + Send>,
    ) -> AgentResult<ExecutionPlan> {
        let system_prompt = self.effective_system_prompt();

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

        let user_message = format!(
            "Objective: {}{}\n\nGenerate a plan as a JSON object with a \"steps\" array. Each step should use \"tool_name\" and \"args\" to call a tool.",
            objective,
            if context.is_empty() {
                String::new()
            } else {
                format!("\nContext: {}", context)
            }
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!("{}{}", user_message, tools_desc)),
        ];

        let mut stream = self
            .llm_client
            .chat_stream(&messages, &[], None, self.response_format().as_ref())
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        on_generating();

        let mut parser = StreamingJsonParser::<PlanStep>::new().with_key("steps");
        let mut full_text = String::new();

        use futures_util::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| AgentError::plan_generation(e.to_string()))?;
            match chunk {
                crate::llm::StreamChunk::Text(text) => {
                    full_text.push_str(&text);
                    on_thought(text.clone());

                    // Try to parse steps from accumulated text
                    let new_steps = parser.process_chunk(&text);
                    for step in new_steps {
                        let idx = parser.accumulated().len() - 1;
                        on_step_parsed(idx, step.id.clone(), step.description.clone());
                    }
                }
                crate::llm::StreamChunk::Stop => break,
                _ => {}
            }
        }

        // If parser didn't extract steps from streaming, try parsing the full response
        let steps = if parser.accumulated().is_empty() {
            let response: Value = serde_json::from_str(&full_text)
                .map_err(|_| AgentError::plan_generation(
                    format!("Failed to parse LLM response as JSON: {}", full_text.chars().take(200).collect::<String>())
                ))?;
            Self::parse_steps(&response)?
        } else {
            parser.accumulated().to_vec()
        };

        // Truncate if over max_steps
        let steps = if steps.len() > self.max_steps {
            tracing::warn!(
                "Plan has {} steps, truncating to {}",
                steps.len(),
                self.max_steps
            );
            steps.into_iter().take(self.max_steps).collect()
        } else {
            steps
        };

        Ok(ExecutionPlan::with_single_phase(Self::next_plan_id(), objective, steps))
    }
}
