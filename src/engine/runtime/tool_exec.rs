use serde_json::Value;
use tokio::sync::broadcast;
use tracing::info_span;

use crate::tool::{ToolContext, ToolOutput, ToolControlFlow};
use crate::types::{AgentResult, AgentError, AgentEvent, SessionId};

use super::AgentRuntime;

pub(super) enum ToolCallResult {
    Continue,
    Break,
}

impl AgentRuntime {
    pub(super) async fn handle_tool_calls<F>(
        &mut self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<ToolCallResult>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let _span = info_span!("tools_exec", session_id = session_id.id).entered();
        tracing::debug!(session_id = session_id.id, tool_count = tool_calls.len(), "executing tools");
        drop(_span);

        let tool_ctx = ToolContext {
            session_id: session_id.clone(),
            event_bus: self.event_bus.clone(),
            llm_client: Some(self.client.clone()),
            session_store: Some(self.session_store.clone()),
        };

        let mut parsed_calls: Vec<(String, String, String, Value)> = Vec::new();
        for (tool_call_id, tool_name, tool_args_json) in tool_calls {
            let args: Value =
                serde_json::from_str(tool_args_json).map_err(|_| AgentError::ToolArgsInvalid {
                    name: tool_name.clone(),
                    raw: tool_args_json.clone(),
                })?;

            self.process_approval(
                session_id,
                tool_name,
                &args,
                tool_args_json,
                event_rx,
                on_event,
            )
            .await?;

            if let Some(policy) = self.tool_policy.as_ref() {
                policy.on_pre_call(tool_name, &args, &tool_ctx);
            }

            parsed_calls.push((tool_call_id.clone(), tool_name.clone(), tool_args_json.clone(), args));
        }

        for (_, tool_name, tool_args_json, _) in &parsed_calls {
            self.emit_event(AgentEvent::ToolCallStarted {
                session_id: session_id.clone(),
                tool_name: tool_name.clone(),
                args_json: tool_args_json.clone(),
            });
        }
        Self::drain_async_events(event_rx, on_event)?;

        {
            let session = self.session_mut_or_err(session_id)?;
            let tc: Vec<(String, String, String)> = parsed_calls
                .iter()
                .map(|(id, name, args_json, _)| (id.clone(), name.clone(), args_json.clone()))
                .collect();
            session.push_assistant_tool_calls(&tc);
        }

        let tools = self.tools.clone();
        let max_output_chars = self.config.max_tool_output_chars;
        let timeout_duration = self.config.tool_timeout_ms.map(std::time::Duration::from_millis);
        let mut results: Vec<(String, String, ToolOutput)> = Vec::new();

        let futures: Vec<_> = parsed_calls
            .iter()
            .map(|(tool_call_id, tool_name, _tool_args_json, args)| {
                let tool_call_id = tool_call_id.clone();
                let tool_name = tool_name.clone();
                let args = args.clone();
                let tool_ctx = tool_ctx.clone();
                let tools = tools.clone();

                async move {
                    let tool_name_owned = tool_name;
                    let tool_call_id_owned = tool_call_id;
                    let tool_name_for_timeout = tool_name_owned.clone();
                    let tool_call_id_for_timeout = tool_call_id_owned.clone();

                    let execute = async {
                        if let Some(tool) = tools.get(&tool_name_owned) {
                            let tool_result = tool.call(&args, &tool_ctx).await.map_err(|e| {
                                AgentError::ToolExecution {
                                    name: tool_name_owned.clone(),
                                    source: Box::new(e),
                                }
                            })?;
                            Ok((tool_call_id_owned, tool_name_owned, tool_result))
                        } else {
                            let name = tool_name_owned.clone();
                            Ok((
                                tool_call_id_owned,
                                tool_name_owned,
                                ToolOutput {
                                    summary: format!("Tool {} not found", name),
                                    raw: None,
                                    control_flow: ToolControlFlow::Break,
                                    truncated: false,
                                },
                            ))
                        }
                    };

                    if let Some(dur) = timeout_duration {
                        match tokio::time::timeout(dur, execute).await {
                            Ok(r) => r,
                            Err(_) => Ok((
                                tool_call_id_for_timeout,
                                tool_name_for_timeout.clone(),
                                ToolOutput {
                                    summary: format!("[Tool Timeout]: 工具 {tool_name_for_timeout} 执行超时"),
                                    raw: None,
                                    control_flow: ToolControlFlow::Continue,
                                    truncated: false,
                                },
                            )),
                        }
                    } else {
                        execute.await
                    }
                }
            })
            .collect();

        for future in futures {
            match future.await {
                Ok((id, name, mut output)) => {
                    if let Some(max_chars) = max_output_chars {
                        if output.summary.len() > max_chars {
                            let truncated_len = max_chars.saturating_sub("...(truncated)".len());
                            output.summary.truncate(truncated_len);
                            output.summary.push_str("...(truncated)");
                            output.truncated = true;
                        }
                    }

                    if let Some(policy) = self.tool_policy.as_ref() {
                        let orig_args = parsed_calls
                            .iter()
                            .find(|(_, n, _, _)| n == &name)
                            .map(|(_, _, _, a)| a)
                            .unwrap_or(&serde_json::Value::Null);
                        policy.on_post_call(&name, orig_args, &output, &tool_ctx);
                    }

                    self.emit_event(AgentEvent::ToolCallFinished {
                        session_id: session_id.clone(),
                        tool_name: name.clone(),
                        summary: output.summary.clone(),
                    });
                    Self::drain_async_events(event_rx, on_event)?;

                    {
                        let session = self.session_mut_or_err(session_id)?;
                        session.push_tool_result(&id, &output.summary);
                    }

                    results.push((id, name, output));
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        let any_continue = results
            .iter()
            .any(|(_, _, r)| matches!(r.control_flow, ToolControlFlow::Continue));
        if any_continue {
            Ok(ToolCallResult::Continue)
        } else {
            Ok(ToolCallResult::Break)
        }
    }
}
