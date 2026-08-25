use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::engine::recovery::ToolErrorAction;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::engine::runtime::tool_engine::ExecutionContext;
use crate::tool::content_text;
use crate::types::{
    AgentError, AgentResult, CheckpointData, CheckpointStep, FinishReason, MessageRole, RunOutcome,
    RuntimeEvent, SessionId,
};

use super::entry::drain_locked;
use super::turn::TurnFlow;
use super::turn_end::TurnEndCtx;
use super::turn_guard::TurnMetrics;

impl RuntimeCore {
    /// Execute tool calls for one LLM turn: truncation guard, execution,
    /// checkpoint emission, and turn-end metrics.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_tool_turn<F>(
        &self,
        session_id: &SessionId,
        user_input: &str,
        turn_count: u32,
        turn_start: std::time::Instant,
        model: &str,
        tool_calls: Vec<(String, String, String)>,
        finish_reason: &FinishReason,
        reasoning_text: String,
        metrics: &TurnMetrics<'_>,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<TurnFlow>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Truncation guard — when the LLM response hit the token limit,
        // tool call arguments may be incomplete. Fail all tool calls
        // without executing them, so the LLM can retry with complete args.
        if finish_reason.is_truncated() {
            tracing::warn!(
                session_id = session_id.id,
                turn = turn_count,
                tool_count = tool_calls.len(),
                "LLM response truncated (finish_reason=length) — tool calls may have incomplete arguments, marking as errors"
            );
            for (tc_id, tc_name, _) in &tool_calls {
                self.with_session_mut(session_id, |session| {
                    session.push_message(
                        MessageRole::Assistant,
                        format!("[would call tool: {}]", tc_name),
                    );
                    session.push_tool_result(
                        tc_id,
                        "Tool call was not executed: the response hit the output token limit, \
                         so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                    );
                })
                .await?;
            }
            return Ok(TurnFlow::Continue);
        }

        tracing::info!(
            session_id = session_id.id,
            turn = turn_count,
            tool_count = tool_calls.len(),
            "handling tool calls"
        );
        self.event_bus.emit(RuntimeEvent::Checkpoint {
            session_id: session_id.clone(),
            checkpoint: CheckpointData {
                session_id: session_id.clone(),
                user_input: user_input.to_string(),
                step: CheckpointStep::BeforeToolCalls {
                    tool_calls: tool_calls.clone(),
                },
                turn_count,
            },
            agent_id: None,
            trace_id: None,
        });

        let tool_start = std::time::Instant::now();
        let tool_call_count = tool_calls.len() as u32;
        let tool_names: Vec<String> = tool_calls.iter().map(|(_, name, _)| name.clone()).collect();

        match self
            .handle_tool_calls(
                session_id,
                &tool_calls,
                event_rx,
                on_event.clone(),
                reasoning_text,
            )
            .await
        {
            Ok(()) => {
                let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms: metrics.ttft_ms,
                    llm_duration_ms: metrics.llm_duration_ms,
                    tool_duration_ms,
                    usage: metrics.usage,
                    text_length: metrics.text_len,
                    has_thinking: metrics.has_thinking,
                    tool_call_count,
                    tools_used: &tool_names,
                    tool_success: tool_call_count,
                    llm_calls: 1,
                    ..TurnEndCtx::new(
                        session_id,
                        turn_count,
                        turn_start,
                        model,
                        user_input,
                        RunOutcome::Completed,
                    )
                })
                .await;
                tracing::info!(
                    session_id = session_id.id,
                    turn = turn_count,
                    "tool calls done, continuing loop"
                );
                let n = tool_calls.len();
                self.with_session_mut(session_id, |session| {
                    session.total_tool_calls += n;
                    session.run_state.record_tool_calls(n);
                })
                .await?;
                self.event_bus.emit(RuntimeEvent::Checkpoint {
                    session_id: session_id.clone(),
                    checkpoint: CheckpointData {
                        session_id: session_id.clone(),
                        user_input: user_input.to_string(),
                        step: CheckpointStep::AfterToolCalls {
                            tool_calls,
                            results: Vec::new(),
                        },
                        turn_count,
                    },
                    agent_id: None,
                    trace_id: None,
                });
                Ok(TurnFlow::Continue)
            }
            Err(e) => {
                let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                let error_msg = e.to_string();
                if let Some(outcome) = self
                    .handle_tool_error(session_id, &tool_calls, e, event_rx, on_event)
                    .await?
                {
                    self.fire_turn_end(TurnEndCtx {
                        ttft_ms: metrics.ttft_ms,
                        llm_duration_ms: metrics.llm_duration_ms,
                        tool_duration_ms,
                        usage: metrics.usage,
                        text_length: metrics.text_len,
                        has_thinking: metrics.has_thinking,
                        tool_call_count,
                        tools_used: &tool_names,
                        tool_failed: tool_call_count,
                        error_message: Some(&error_msg),
                        llm_calls: 1,
                        ..TurnEndCtx::new(
                            session_id,
                            turn_count,
                            turn_start,
                            model,
                            user_input,
                            RunOutcome::Failed {
                                error: error_msg.clone(),
                            },
                        )
                    })
                    .await;
                    return Ok(TurnFlow::Done(outcome));
                }
                // Retry: record metrics for the failed attempt
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms: metrics.ttft_ms,
                    llm_duration_ms: metrics.llm_duration_ms,
                    tool_duration_ms,
                    usage: metrics.usage,
                    text_length: metrics.text_len,
                    has_thinking: metrics.has_thinking,
                    tool_call_count,
                    tools_used: &tool_names,
                    tool_failed: tool_call_count,
                    error_message: Some(&error_msg),
                    llm_calls: 1,
                    ..TurnEndCtx::new(
                        session_id,
                        turn_count,
                        turn_start,
                        model,
                        user_input,
                        RunOutcome::Failed {
                            error: error_msg.clone(),
                        },
                    )
                })
                .await;
                Ok(TurnFlow::Continue)
            }
        }
    }

    pub(super) async fn handle_tool_error<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        e: AgentError,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<Option<RunOutcome>>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let config = self.config_snapshot_async().await;

        if e.is_cancelled() {
            // Don't emit RunFinished when cancelled — RunCancelled will be emitted by run_turn
            // But still drain pending events and persist session
            drain_locked(event_rx, &on_event)?;
            let session = self.session_manager.session_or_err(session_id).await?;
            if let Err(e) = self.session_manager.session_store().save(&session).await {
                tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                if config.execution.fail_on_persist_error {
                    return Err(AgentError::internal(format!(
                        "Session persistence failed: {e}"
                    )));
                }
            }
            return Err(e);
        }

        let names = failing_tool_names(tool_calls, &e);
        let error_text = e.to_string();
        let retry_prompt_template: Option<String> = config.tool.tool_error_retry_prompt.clone();

        // 用户主动拒绝（输入 n）→ 立即停止，不走 recovery 重试逻辑
        if matches!(e, AgentError::ApprovalDenied { .. }) {
            tracing::info!(
                session_id = session_id.id,
                "user rejected tool call, stopping immediately"
            );
            let error_summary = if config.language == crate::types::Language::Zh {
                format!("❌ 用户拒绝执行: {}", e)
            } else {
                format!("❌ User rejected: {}", e)
            };
            self.with_session_mut(session_id, |session| {
                session.close_dangling_tool_calls(&error_summary);
                session.remove_ephemeral_messages();
            })
            .await?;
            self.event_bus.emit(RuntimeEvent::RunFinished {
                session_id: session_id.clone(),
                agent_id: None,
                trace_id: None,
            });
            drain_locked(event_rx, &on_event)?;
            let session = self.session_manager.session_or_err(session_id).await?;
            if let Err(e) = self.session_manager.session_store().save(&session).await {
                tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
            }
            return Ok(Some(RunOutcome::Completed));
        }

        let action = self
            .tool_engine
            .error_recovery()
            .on_error(session_id, &names, &e)?;
        match action {
            ToolErrorAction::Stop => {
                // Close any dangling tool calls in the session before stopping
                let error_summary = if config.language == crate::types::Language::Zh {
                    format!("❌ 执行失败: {}", e)
                } else {
                    format!("❌ Tool execution failed: {}", e)
                };
                // Emit ToolCallFinished so TUI shows the failure before RunFinished
                self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: names.join(", "),
                    summary: error_summary.clone(),
                    agent_id: None,
                    trace_id: None,
                    denied: false,
                });
                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&error_summary);
                    session.remove_ephemeral_messages();
                })
                .await?;
                self.event_bus.emit(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                drain_locked(event_rx, &on_event)?;
                let session = self.session_manager.session_or_err(session_id).await?;
                if let Err(e) = self.session_manager.session_store().save(&session).await {
                    tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                    if config.execution.fail_on_persist_error {
                        return Err(AgentError::internal(format!(
                            "Session persistence failed: {e}"
                        )));
                    }
                }
                Ok(Some(RunOutcome::Failed {
                    error: format!("Tool execution failed: {}", e),
                }))
            }
            ToolErrorAction::RetryWithHistory { errors } => {
                let tool_name = names.join(", ");
                let error_history = format!(
                    "The tool `{}` has failed {} consecutive times. \
                     Error history:\n\n{}\n\n\
                     Please analyze these errors and either switch to a different \
                     approach or explain the failure to the user.",
                    tool_name,
                    errors.len(),
                    errors
                        .iter()
                        .enumerate()
                        .map(|(i, e)| format!("{}. {}", i + 1, e))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                );

                let summary = if config.language == crate::types::Language::Zh {
                    format!(
                        "⚠️ {} 连续失败 {} 次，已将错误历史发给 LLM 评估",
                        tool_name,
                        errors.len()
                    )
                } else {
                    format!(
                        "⚠️ {} failed {} consecutive times, error history sent to LLM",
                        tool_name,
                        errors.len()
                    )
                };

                // Emit ToolCallFinished so TUI shows the recovery action
                self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: tool_name.clone(),
                    summary: summary.clone(),
                    agent_id: None,
                    trace_id: None,
                    denied: false,
                });

                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&summary);
                    session.push_message(MessageRole::User, error_history);
                })
                .await?;
                Ok(None)
            }
            ToolErrorAction::Retry => {
                let retry_prompt = match &retry_prompt_template {
                    Some(template) => template
                        .replace("{tool_names}", &names.join(", "))
                        .replace("{error}", &error_text),
                    None => format!(
                        "Tool calls failed: {}\nError: {}\nPlease analyze the error and adjust your approach.",
                        names.join(", "),
                        error_text,
                    ),
                };

                let error_summary = if config.language == crate::types::Language::Zh {
                    format!("❌ 执行失败: {}", error_text)
                } else {
                    format!("❌ Tool execution failed: {}", error_text)
                };
                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&error_summary);
                    session.push_message(MessageRole::User, retry_prompt);
                })
                .await?;
                Ok(None)
            }
        }
    }

    pub(super) async fn handle_tool_calls<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
        reasoning: String,
    ) -> AgentResult<()>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let tool_names: Vec<&str> = tool_calls
            .iter()
            .map(|(_, name, _)| name.as_str())
            .collect();
        tracing::debug!(
            session_id = session_id.id,
            ?tool_names,
            "handle tool calls start"
        );

        let config = self.config_snapshot_async().await;

        let ctx = ExecutionContext {
            session_manager: self.session_manager.clone(),
            llm_client: Some(self.llm_engine.get_provider()),
            language: config.language.clone(),
            tool_timeout_ms: config.tool.tool_timeout_ms,
            max_output_chars: config.tool.max_tool_output_chars,
            cancel_token: self.cancel_token(),
        };

        // Orchestrate: approval check + execution for all tool calls.
        // Approval denial is handled inside process_approval (pushes fake tool state
        // and returns Err). We only push real tool calls on success.
        let outcome = self
            .tool_engine
            .orchestrate(session_id, tool_calls, &ctx, event_rx, on_event.clone())
            .await?;

        // Push assistant tool calls to session after all approvals pass
        {
            let tc: Vec<(String, String, String)> = tool_calls.to_vec();
            self.with_session_mut(session_id, |session| {
                let r = if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning.clone())
                };
                session.push_assistant_tool_calls(&tc, r);
            })
            .await?;
        }

        // Process successful results: push to session.
        for result in &outcome.results {
            self.with_session_mut(session_id, |session| {
                session.push_tool_result(&result.id, content_text(&result.output));
            })
            .await?;
        }

        // Push each failure's own error text so the model sees exactly what broke
        // (not just the first failure's message) and every tool call has a result.
        for failure in &outcome.failures {
            let summary = failure.error.to_string();
            self.with_session_mut(session_id, |session| {
                session.push_tool_result(&failure.id, summary);
            })
            .await?;
        }

        // Surface the first per-call failure back to the caller so the existing
        // `handle_tool_error` path can run recovery and feed the model a retry prompt.
        // Recovery only counts this first failing tool here — an undercount in the
        // rare multi-failure batch, which errs on the lenient (never false-Stop) side.
        if let Some(first) = outcome.failures.into_iter().next() {
            return Err(first.error);
        }

        Ok(())
    }
}

/// Names of the tools that actually failed in a batch of tool calls.
///
/// Tool-level errors carry the failing tool's name on the error itself, so a single
/// bad call in a large batch yields exactly one name (not every tool in the batch,
/// which would inflate the consecutive-failure counter and trip the Stop threshold).
/// Unknown error types conservatively fall back to the full batch.
fn failing_tool_names(tool_calls: &[(String, String, String)], error: &AgentError) -> Vec<String> {
    match error {
        AgentError::ToolArgsInvalid { name, .. } => vec![name.clone()],
        AgentError::ToolExecution { name, .. } => vec![name.clone()],
        _ => tool_calls.iter().map(|(_, n, _)| n.clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failing_tool_names_isolates_single_failing_tool() {
        let tool_calls = vec![
            (
                "id1".to_string(),
                "write_file".to_string(),
                "{}".to_string(),
            ),
            (
                "id2".to_string(),
                "write_file".to_string(),
                "{}".to_string(),
            ),
            (
                "id3".to_string(),
                "write_file".to_string(),
                "{}".to_string(),
            ),
        ];

        // One bad call in a batch of three must yield one name, not three.
        let err = AgentError::ToolArgsInvalid {
            name: "write_file".to_string(),
            raw: "truncated-json".to_string(),
        };
        assert_eq!(
            failing_tool_names(&tool_calls, &err),
            vec!["write_file".to_string()]
        );
    }

    #[test]
    fn failing_tool_names_extracts_execution_failure_name() {
        let tool_calls = vec![
            ("id1".to_string(), "bash".to_string(), "{}".to_string()),
            ("id2".to_string(), "bash".to_string(), "{}".to_string()),
        ];
        let err = AgentError::ToolExecution {
            name: "bash".to_string(),
            source: Box::new(AgentError::internal("boom")),
        };
        assert_eq!(
            failing_tool_names(&tool_calls, &err),
            vec!["bash".to_string()]
        );
    }

    #[test]
    fn failing_tool_names_falls_back_to_full_batch_for_unknown_error() {
        let tool_calls = vec![
            ("id1".to_string(), "a".to_string(), "{}".to_string()),
            ("id2".to_string(), "b".to_string(), "{}".to_string()),
        ];
        let err = AgentError::internal("unexpected");
        assert_eq!(
            failing_tool_names(&tool_calls, &err),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
