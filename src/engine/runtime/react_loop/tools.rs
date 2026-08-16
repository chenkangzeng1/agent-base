use tokio::sync::broadcast;

use crate::engine::recovery::ToolErrorAction;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::engine::runtime::tool_engine::ExecutionContext;
use crate::tool::content_text;
use crate::types::{AgentError, AgentResult, MessageRole, RunOutcome, RuntimeEvent, SessionId};

impl RuntimeCore {
    pub(super) async fn handle_tool_error<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        e: AgentError,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<Option<RunOutcome>>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let config = self.config_snapshot_async().await;

        if e.is_cancelled() {
            // Don't emit RunFinished when cancelled — RunCancelled will be emitted by run_turn
            // But still drain pending events and persist session
            EventBus::drain_async_events(event_rx, on_event)?;
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
            EventBus::drain_async_events(event_rx, on_event)?;
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
                EventBus::drain_async_events(event_rx, on_event)?;
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
        on_event: &mut F,
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
            llm_client: Some(self.llm_engine.get_client()),
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
            .orchestrate(session_id, tool_calls, &ctx, event_rx, on_event)
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
            ("id1".to_string(), "write_file".to_string(), "{}".to_string()),
            ("id2".to_string(), "write_file".to_string(), "{}".to_string()),
            ("id3".to_string(), "write_file".to_string(), "{}".to_string()),
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
        assert_eq!(failing_tool_names(&tool_calls, &err), vec!["bash".to_string()]);
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
