use async_trait::async_trait;

use crate::LogEntry;

#[async_trait]
pub trait LogSink: Send + Sync {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()>;

    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
