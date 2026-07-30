use std::process::Stdio;
use std::time::Duration;

use agent_base::{AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};
use async_trait::async_trait;
use serde_json::{Value, json};

/// Local shell command execution tool.
///
/// Executes arbitrary commands via `sh -c`, with support for timeout,
/// cancellation, and working directory.
pub struct LocalShellTool {
    timeout_ms: u64,
}

impl LocalShellTool {
    pub fn new(timeout_ms: u64) -> Self {
        Self { timeout_ms }
    }
}

fn format_result(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    timed_out: bool,
) -> String {
    let stdout = stdout.trim();
    let stderr = stderr.trim();

    if timed_out {
        return format!(
            "[Command Timed Out]\ncommand: {}\nstdout:\n{}\nstderr:\n{}",
            command,
            if stdout.is_empty() { "(empty)" } else { stdout },
            if stderr.is_empty() { "(empty)" } else { stderr },
        );
    }

    match exit_code {
        Some(0) => match (stdout.is_empty(), stderr.is_empty()) {
            (true, true) => "Command executed successfully with no output.".to_string(),
            (false, true) => stdout.to_string(),
            (true, false) => format!("stderr:\n{}", stderr),
            (false, false) => format!("stdout:\n{}\n\nstderr:\n{}", stdout, stderr),
        },
        Some(code) => format!(
            "[Command Failed (exit code: {})]\ncommand: {}\nstdout:\n{}\nstderr:\n{}",
            code,
            command,
            if stdout.is_empty() { "(empty)" } else { stdout },
            if stderr.is_empty() { "(empty)" } else { stderr },
        ),
        None => format!(
            "[Command Terminated]\ncommand: {}\nstdout:\n{}\nstderr:\n{}",
            command,
            if stdout.is_empty() { "(empty)" } else { stdout },
            if stderr.is_empty() { "(empty)" } else { stderr },
        ),
    }
}

#[async_trait]
impl Tool for LocalShellTool {
    fn name(&self) -> &'static str {
        "execute_command"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "execute_command",
                "description": "Execute a shell command locally. Use for file operations, code compilation, Git operations, system info queries, etc. For commands that may produce large output, consider limiting lines (e.g. journalctl -n 50, grep ... | head -n 30).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute. For commands that may produce large output, consider limiting lines: cat large files with | tail -n 30, find / ls -R with | head -n 50, grep over large scope with | head -n 30."
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "Working directory. Uses the current directory if not specified."
                        }
                    },
                    "required": ["command"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        if command.is_empty() {
            return Ok(ToolOutput {
                summary: "[Error]: No command provided.".to_string(),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncation: None,
            });
        }

        tracing::info!(command = %command, timeout_ms = self.timeout_ms, "execute_command start");

        let working_dir = args.get("working_dir").and_then(Value::as_str);

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        // spawn + timeout + kill pattern: explicitly kill child process on timeout
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, command = %command, "execute_command: spawn failed");
                return Ok(ToolOutput {
                    summary: format!("[Error]: Command execution failed: {}", e),
                    raw: Some(json!({ "error": e.to_string(), "command": command })),
                    control_flow: ToolControlFlow::Continue,
                    truncation: None,
                });
            }
        };

        let pid = child.id();
        let sleep = tokio::time::sleep(Duration::from_millis(self.timeout_ms));
        tokio::pin!(sleep);

        let output = tokio::select! {
            result = child.wait_with_output() => {
                match result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let exit_code = output.status.code();

                        tracing::info!(
                            command = %command,
                            exit_code = exit_code,
                            stdout_len = stdout.len(),
                            stderr_len = stderr.len(),
                            "execute_command: done"
                        );

                        let summary = format_result(&command, &stdout, &stderr, exit_code, false);
                        Ok(ToolOutput {
                            summary,
                            raw: Some(json!({
                                "command": command,
                                "exit_code": exit_code,
                                "stdout": stdout,
                                "stderr": stderr,
                                "timed_out": false,
                            })),
                            control_flow: ToolControlFlow::Continue,
                            truncation: None,
                        })
                    }
                    Err(e) => {
                        tracing::error!(error = %e, command = %command, "execute_command: wait failed");
                        Ok(ToolOutput {
                            summary: format!("[Error]: Command execution failed: {}", e),
                            raw: Some(json!({ "error": e.to_string(), "command": command })),
                            control_flow: ToolControlFlow::Continue,
                            truncation: None,
                        })
                    }
                }
            }
            _ = &mut sleep => {
                // Timeout — kill the child process by pid (child has been moved by wait_with_output)
                if let Some(pid) = pid {
                    let _ = tokio::process::Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }
                tracing::warn!(command = %command, timeout_ms = self.timeout_ms, "execute_command: timed out and killed");
                Ok(ToolOutput {
                    summary: format!(
                        "[Command Timed Out after {}ms]\ncommand: {}",
                        self.timeout_ms, command
                    ),
                    raw: Some(json!({
                        "command": command,
                        "timed_out": true,
                        "timeout_ms": self.timeout_ms,
                    })),
                    control_flow: ToolControlFlow::Continue,
                    truncation: None,
                })
            }
        };

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_result_success() {
        let result = format_result("echo hello", "hello", "", Some(0), false);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_format_result_no_output() {
        let result = format_result("true", "", "", Some(0), false);
        assert!(result.contains("no output"));
    }

    #[test]
    fn test_format_result_failure() {
        let result = format_result("false", "", "error", Some(1), false);
        assert!(result.contains("Command Failed"));
        assert!(result.contains("exit code: 1"));
    }

    #[test]
    fn test_format_result_timeout() {
        let result = format_result("sleep 100", "", "", None, true);
        assert!(result.contains("Command Timed Out"));
    }

    #[test]
    fn test_name() {
        let tool = LocalShellTool::new(30000);
        assert_eq!(tool.name(), "execute_command");
    }

    #[test]
    fn test_definition() {
        let tool = LocalShellTool::new(30000);
        let def = tool.definition();
        assert_eq!(def["function"]["name"], "execute_command");
        assert!(
            def["function"]["description"]
                .as_str()
                .unwrap()
                .contains("shell")
        );
    }
}
