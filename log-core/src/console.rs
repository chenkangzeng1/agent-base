use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::{LogEntry, LogLevel, LogSink};

pub struct ConsoleSink;

impl ConsoleSink {
    pub fn new() -> Self {
        Self
    }
}

impl ConsoleSink {
    fn format_entry(&self, entry: &LogEntry) -> String {
        let timestamp = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
        let context_str = if entry.context.is_null() {
            String::new()
        } else {
            format!(" {}", entry.context)
        };

        format!(
            "[{}] [{:5}] [{}] {}{}",
            timestamp, entry.level, entry.module, entry.message, context_str
        )
    }
}

#[async_trait]
impl LogSink for ConsoleSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        let line = self.format_entry(entry);
        let colored = match entry.level {
            LogLevel::Error => format!("\x1b[31m{}\x1b[0m", line),
            LogLevel::Warn => format!("\x1b[33m{}\x1b[0m", line),
            LogLevel::Debug => format!("\x1b[90m{}\x1b[0m", line),
            LogLevel::Info => line,
        };
        let mut stderr = tokio::io::stderr();
        stderr.write_all(colored.as_bytes()).await?;
        stderr.write_all(b"\n").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_entry() {
        let sink = ConsoleSink::new();
        let entry = LogEntry::new(LogLevel::Info, "test", "hello", json!(null));
        let formatted = sink.format_entry(&entry);
        assert!(formatted.contains("[INFO]"));
        assert!(formatted.contains("[test]"));
        assert!(formatted.contains("hello"));
    }

    #[test]
    fn test_format_entry_with_context() {
        let sink = ConsoleSink::new();
        let entry = LogEntry::new(LogLevel::Warn, "mod", "msg", json!({"k": "v"}));
        let formatted = sink.format_entry(&entry);
        assert!(formatted.contains(r#"{"k":"v"}"#));
    }
}
