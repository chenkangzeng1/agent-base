use serde_json::Value;

use crate::{LogEntry, LogLevel, LogSink};

pub struct Logger {
    sinks: Vec<Box<dyn LogSink>>,
    min_level: LogLevel,
}

impl Logger {
    pub fn builder() -> LoggerBuilder {
        LoggerBuilder::new()
    }

    pub async fn log(
        &self,
        level: LogLevel,
        module: &'static str,
        message: impl Into<String>,
        context: Value,
    ) {
        if level < self.min_level {
            return;
        }

        let entry = LogEntry::new(level, module, message, context);
        for sink in &self.sinks {
            let _ = sink.write(&entry).await;
        }
    }

    pub async fn debug(&self, module: &'static str, message: impl Into<String>, context: Value) {
        self.log(LogLevel::Debug, module, message, context).await;
    }

    pub async fn info(&self, module: &'static str, message: impl Into<String>, context: Value) {
        self.log(LogLevel::Info, module, message, context).await;
    }

    pub async fn warn(&self, module: &'static str, message: impl Into<String>, context: Value) {
        self.log(LogLevel::Warn, module, message, context).await;
    }

    pub async fn error(&self, module: &'static str, message: impl Into<String>, context: Value) {
        self.log(LogLevel::Error, module, message, context).await;
    }

    pub async fn flush(&self) {
        for sink in &self.sinks {
            let _ = sink.flush().await;
        }
    }
}

pub struct LoggerBuilder {
    sinks: Vec<Box<dyn LogSink>>,
    min_level: LogLevel,
}

impl LoggerBuilder {
    pub fn new() -> Self {
        Self {
            sinks: Vec::new(),
            min_level: LogLevel::Debug,
        }
    }

    pub fn min_level(mut self, level: LogLevel) -> Self {
        self.min_level = level;
        self
    }

    pub fn console(mut self) -> Self {
        self.sinks.push(Box::new(crate::ConsoleSink::new()));
        self
    }

    pub fn file(mut self, sink: crate::FileSink) -> Self {
        self.sinks.push(Box::new(sink));
        self
    }

    pub fn sink(mut self, sink: Box<dyn LogSink>) -> Self {
        self.sinks.push(sink);
        self
    }

    pub fn build(self) -> Logger {
        Logger {
            sinks: self.sinks,
            min_level: self.min_level,
        }
    }
}

impl Default for LoggerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_min_level_filters_debug() {
        let logger = Logger::builder()
            .console()
            .min_level(LogLevel::Info)
            .build();

        // debug should be filtered out, this should not panic
        logger
            .debug("test", "this should be filtered", json!(null))
            .await;
    }

    #[tokio::test]
    async fn test_min_level_allows_info() {
        let logger = Logger::builder()
            .console()
            .min_level(LogLevel::Info)
            .build();

        logger.info("test", "this should pass", json!(null)).await;
    }

    #[tokio::test]
    async fn test_log_method() {
        let logger = Logger::builder()
            .console()
            .min_level(LogLevel::Debug)
            .build();

        logger
            .log(LogLevel::Warn, "test", "direct log call", json!({"key": 1}))
            .await;
    }

    #[tokio::test]
    async fn test_custom_sink() {
        use async_trait::async_trait;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        struct MockSink {
            entries: Arc<Mutex<Vec<LogEntry>>>,
        }

        #[async_trait]
        impl LogSink for MockSink {
            async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
                self.entries.lock().await.push(entry.clone());
                Ok(())
            }
        }

        let entries = Arc::new(Mutex::new(Vec::new()));
        let mock = MockSink {
            entries: entries.clone(),
        };

        let logger = Logger::builder().sink(Box::new(mock)).build();

        logger.info("test", "mock entry", json!(null)).await;
        logger.warn("test", "warning", json!(null)).await;

        let captured = entries.lock().await;
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].message, "mock entry");
        assert_eq!(captured[1].message, "warning");
    }

    #[tokio::test]
    async fn test_all_log_levels() {
        use async_trait::async_trait;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        struct MockSink {
            entries: Arc<Mutex<Vec<LogEntry>>>,
        }

        #[async_trait]
        impl LogSink for MockSink {
            async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
                self.entries.lock().await.push(entry.clone());
                Ok(())
            }
        }

        let entries = Arc::new(Mutex::new(Vec::new()));
        let mock = MockSink {
            entries: entries.clone(),
        };

        let logger = Logger::builder()
            .sink(Box::new(mock))
            .min_level(LogLevel::Debug)
            .build();

        logger.debug("t", "debug msg", json!(null)).await;
        logger.info("t", "info msg", json!(null)).await;
        logger.warn("t", "warn msg", json!(null)).await;
        logger.error("t", "error msg", json!(null)).await;

        let captured = entries.lock().await;
        assert_eq!(captured.len(), 4);
        assert_eq!(captured[0].level, LogLevel::Debug);
        assert_eq!(captured[1].level, LogLevel::Info);
        assert_eq!(captured[2].level, LogLevel::Warn);
        assert_eq!(captured[3].level, LogLevel::Error);
    }

    #[tokio::test]
    async fn test_flush() {
        let logger = Logger::builder().console().build();
        logger.flush().await;
    }
}
