use std::fmt;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::sync::RwLock;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::{ConsoleSink, FileSink, LogEntry, LogLevel, LogSink};

/// A handle that can add sinks to a [`LogCoreLayer`] at runtime.
///
/// Obtained via [`LogCoreLayer::sink_handle()`].
#[derive(Clone)]
pub struct SinkHandle(Arc<RwLock<Vec<Box<dyn LogSink>>>>);

impl SinkHandle {
    /// Add a new sink. All subsequent log events will be written to it.
    pub async fn add_sink(&self, sink: Box<dyn LogSink>) {
        self.0.write().await.push(sink);
    }
}

pub struct LogCoreLayer {
    sinks: Arc<RwLock<Vec<Box<dyn LogSink>>>>,
    min_level: LogLevel,
}

impl LogCoreLayer {
    pub fn new(sinks: Vec<Box<dyn LogSink>>, min_level: LogLevel) -> Self {
        Self {
            sinks: Arc::new(RwLock::new(sinks)),
            min_level,
        }
    }

    pub fn console(min_level: LogLevel) -> Self {
        Self::new(vec![Box::new(ConsoleSink::new())], min_level)
    }

    pub async fn file(path: &str, min_level: LogLevel) -> anyhow::Result<Self> {
        let sink = FileSink::new(path).await?;
        Ok(Self::new(vec![Box::new(sink)], min_level))
    }

    pub async fn console_and_file(path: &str, min_level: LogLevel) -> anyhow::Result<Self> {
        let file_sink = FileSink::new(path).await?;
        Ok(Self::new(
            vec![Box::new(ConsoleSink::new()), Box::new(file_sink)],
            min_level,
        ))
    }

    /// Get a handle that can add sinks at runtime.
    pub fn sink_handle(&self) -> SinkHandle {
        SinkHandle(self.sinks.clone())
    }
}

impl<S> Layer<S> for LogCoreLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();

        let level = match *metadata.level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG | tracing::Level::TRACE => LogLevel::Debug,
        };

        if level < self.min_level {
            return;
        }

        let mut visitor = JsonVisitor {
            fields: Map::new(),
            message: String::new(),
        };
        event.record(&mut visitor);

        let message = if visitor.message.is_empty() {
            metadata.name().to_string()
        } else {
            visitor.message
        };

        let context = Value::Object(visitor.fields);
        let entry = LogEntry::new(level, metadata.target(), message, context);
        let sinks = self.sinks.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let guard = sinks.read().await;
                for sink in guard.iter() {
                    let _ = sink.write(&entry).await;
                }
            });
        }
    }
}

struct JsonVisitor {
    fields: Map<String, Value>,
    message: String,
}

impl Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        let s = format!("{value:?}");
        if field.name() == "message" {
            self.message = strip_quotes(&s).to_string();
        } else {
            self.fields
                .insert(field.name().to_string(), Value::String(s));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .insert(field.name().to_string(), Value::String(value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            Value::Number(serde_json::Number::from(value)),
        );
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            Value::Number(serde_json::Number::from(value)),
        );
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
