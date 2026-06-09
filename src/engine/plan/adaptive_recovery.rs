use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::LlmClient;
use crate::types::{AgentError, AgentResult, ChatMessage, PlanStep, RecoveryAction, RecoveryContext};

use super::traits::AdaptiveRecoveryStrategy;

/// Default LLM-driven adaptive recovery strategy.
///
/// The framework's orchestration loop already guarantees `max_alternatives` /
/// `max_replans` hard limits, so this implementation makes soft decisions based
/// on the quota information in [`RecoveryContext`]:
/// - Alternative budget remaining → ask LLM to generate an alternative step
/// - Alternative budget exhausted, replan budget remaining → ask LLM to replan
/// - Both exhausted → Abort
///
/// # Prompt customization
///
/// Use [`with_alternative_prompt`](Self::with_alternative_prompt) and
/// [`with_replan_prompt`](Self::with_replan_prompt) to override the default
/// system prompts, matching the API style of
/// [`LlmPlanGenerator`](super::LlmPlanGenerator).
pub struct LlmAdaptiveRecovery {
    llm_client: Arc<dyn LlmClient>,
    alternative_prompt: Option<String>,
    replan_prompt: Option<String>,
}

impl LlmAdaptiveRecovery {
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            llm_client,
            alternative_prompt: None,
            replan_prompt: None,
        }
    }

    /// Override the default system prompt for alternative step generation.
    pub fn with_alternative_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.alternative_prompt = Some(prompt.into());
        self
    }

    /// Override the default system prompt for replan generation.
    pub fn with_replan_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.replan_prompt = Some(prompt.into());
        self
    }

    /// Call the LLM and extract the text content from the response.
    async fn llm_text(&self, system_prompt: &str, user_prompt: &str) -> AgentResult<String> {
        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(user_prompt),
        ];
        let response = self
            .llm_client
            .chat(&messages, &[], None, None)
            .await?;
        Ok(extract_text_from_response(&response))
    }

    /// Ask the LLM to generate an alternative step using a different tool or parameters.
    async fn try_alternative(&self, ctx: &RecoveryContext) -> AgentResult<RecoveryAction> {
        let system_prompt = self.alternative_prompt.as_deref().unwrap_or(
            "You are a step recovery module in a task execution engine.\n\
             Generate an ALTERNATIVE step that achieves the SAME goal using a DIFFERENT tool or different parameters.\n\
             Output JSON only. If no viable alternative exists, output: {\"abort\": true, \"reason\": \"...\"}",
        );

        let user_prompt = format!(
            "Failed step: {desc}\n\
             Original tool: {tool}\n\
             Args: {args}\n\
             Error: {error}\n\
             Retry attempts: {retries}\n\
             Previous alternative attempts: {alts}\n\n\
             Available tools:\n{tools}\n\n\
             Previous step results:\n{outputs}\n\n\
             Generate ONE alternative step as JSON:\n\
             {{\"id\": \"{orig_id}-alt-{attempt}\", \"description\": \"...\", \"tool_name\": \"...\", \"args\": {{...}}}}\n\
             If no viable alternative: {{\"abort\": true, \"reason\": \"...\"}}",
            desc = ctx.failed_step.description,
            tool = ctx.failed_step.payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown"),
            args = ctx.failed_step.payload.get("args").unwrap_or(&json!({})),
            error = ctx.error,
            retries = ctx.retry_count,
            alts = ctx.alternative_count,
            tools = serde_json::to_string_pretty(&ctx.available_tools).unwrap_or_default(),
            outputs = ctx.step_outputs,
            orig_id = ctx.failed_step.id,
            attempt = ctx.alternative_count + 1,
        );

        let text = self.llm_text(system_prompt, &user_prompt).await?;
        let parsed: Value = parse_json_from_text(&text)
            .map_err(|e| AgentError::json(format!("Failed to parse alternative step: {e}")))?;

        if parsed.get("abort").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(RecoveryAction::Abort);
        }

        let step = parse_step_from_json(&parsed)?;
        Ok(RecoveryAction::Alternative {
            step,
            root_step_id: ctx.root_step_id.clone(),
        })
    }

    /// Ask the LLM to replan the remaining steps based on current progress.
    async fn try_replan(&self, ctx: &RecoveryContext) -> AgentResult<RecoveryAction> {
        let system_prompt = self.replan_prompt.as_deref().unwrap_or(
            "You are a plan replanning module in a task execution engine.\n\
             Based on completed steps and the failure, generate a NEW sequence of steps to achieve the plan objective.\n\
             Output JSON only. If the objective is unreachable, output: {\"abort\": true, \"reason\": \"...\"}",
        );

        let completed_steps: Vec<Value> = ctx
            .plan
            .all_steps()
            .filter(|s| matches!(s.status, crate::types::StepStatus::Completed))
            .map(|s| json!({"id": s.id, "description": s.description}))
            .collect();

        let user_prompt = format!(
            "Plan objective: {objective}\n\
             Completed steps: {completed}\n\
             Failed step: {desc}\n\
             Error: {error}\n\
             Retry attempts: {retries}\n\
             Alternative attempts: {alts}\n\
             Previous replan attempts: {replans}\n\n\
             Available tools:\n{tools}\n\n\
             Step outputs:\n{outputs}\n\n\
             Generate a new step sequence as JSON array:\n\
             [{{\"id\": \"...\", \"description\": \"...\", \"tool_name\": \"...\", \"args\": {{...}}}}]\n\
             If objective is unreachable: {{\"abort\": true, \"reason\": \"...\"}}",
            objective = ctx.plan.objective,
            completed = serde_json::to_string(&completed_steps).unwrap_or_default(),
            desc = ctx.failed_step.description,
            error = ctx.error,
            retries = ctx.retry_count,
            alts = ctx.alternative_count,
            replans = ctx.replan_count,
            tools = serde_json::to_string_pretty(&ctx.available_tools).unwrap_or_default(),
            outputs = ctx.step_outputs,
        );

        let text = self.llm_text(system_prompt, &user_prompt).await?;
        let parsed: Value = parse_json_from_text(&text)
            .map_err(|e| AgentError::json(format!("Failed to parse replan response: {e}")))?;

        if let Some(abort) = parsed.get("abort").and_then(|v| v.as_bool()) {
            if abort {
                return Ok(RecoveryAction::Abort);
            }
        }

        // Expect a JSON array of steps
        let steps_array = parsed
            .as_array()
            .ok_or_else(|| AgentError::json("Replan response must be a JSON array of steps"))?;

        let steps: Vec<PlanStep> = steps_array
            .iter()
            .map(|v| parse_step_from_json(v))
            .collect::<AgentResult<Vec<_>>>()?;

        Ok(RecoveryAction::Replan {
            steps,
            clear_future_phases: true,
        })
    }
}

/// Extract text content from an LLM response JSON (handles OpenAI/Anthropic formats).
fn extract_text_from_response(response: &Value) -> String {
    // OpenAI format: choices[0].message.content
    if let Some(text) = response
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
    {
        return text.to_string();
    }

    // Anthropic format: content[0].text
    if let Some(content) = response.get("content").and_then(|v| v.as_array()) {
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    return text.to_string();
                }
            }
        }
    }

    // Fallback: try to use the whole response as text
    response.to_string()
}

/// Parse JSON from LLM text output, stripping markdown code blocks if present.
fn parse_json_from_text(text: &str) -> AgentResult<Value> {
    let trimmed = text.trim();

    // Try direct parse
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }

    // Try stripping markdown code blocks: ```json ... ``` or ``` ... ```
    let stripped = if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        inner
    } else {
        trimmed
    };

    serde_json::from_str::<Value>(stripped)
        .map_err(|e| AgentError::json(format!("JSON parse error: {e}")))
}

/// Parse a single PlanStep from a JSON value.
fn parse_step_from_json(v: &Value) -> AgentResult<PlanStep> {
    let id = v
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::json("Step JSON missing 'id' field"))?;
    let description = v
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::json("Step JSON missing 'description' field"))?;
    let tool_name = v
        .get("tool_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentError::json("Step JSON missing 'tool_name' field"))?;
    let args = v.get("args").cloned().unwrap_or(json!({}));

    Ok(PlanStep::tool_call(id, description, tool_name, args))
}

#[async_trait]
impl AdaptiveRecoveryStrategy for LlmAdaptiveRecovery {
    async fn recover(&self, ctx: &RecoveryContext) -> AgentResult<RecoveryAction> {
        // Progressive decision based on context quotas
        // (framework also hard-enforces these limits)
        if ctx.alternative_count < ctx.max_alternatives {
            return self.try_alternative(ctx).await;
        }
        if ctx.replan_count < ctx.max_replans {
            return self.try_replan(ctx).await;
        }
        Ok(RecoveryAction::Abort)
    }
}
