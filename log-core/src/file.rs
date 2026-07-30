use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::{LogEntry, LogSink};

const DEFAULT_MAX_SIZE: u64 = 10 * 1024 * 1024;
const DEFAULT_MAX_FILES: usize = 5;

pub struct FileSink {
    base_path: PathBuf,
    max_size: u64,
    max_files: usize,
    state: Arc<Mutex<FileSinkState>>,
}

struct FileSinkState {
    file: tokio::fs::File,
    current_size: u64,
}

impl FileSink {
    pub async fn new(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        Self::with_config(path, DEFAULT_MAX_SIZE, DEFAULT_MAX_FILES).await
    }

    pub async fn with_config(
        path: impl Into<PathBuf>,
        max_size: u64,
        max_files: usize,
    ) -> anyhow::Result<Self> {
        let base_path = path.into();
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&base_path)
            .await?;
        let metadata = file.metadata().await?;
        let current_size = metadata.len();

        Ok(Self {
            base_path,
            max_size,
            max_files,
            state: Arc::new(Mutex::new(FileSinkState { file, current_size })),
        })
    }

    fn format_entry(&self, entry: &LogEntry) -> String {
        let timestamp = entry.timestamp.format("%Y-%m-%d %H:%M:%S");
        let context_str = if entry.context.is_null() {
            String::new()
        } else {
            format!(" {}", entry.context)
        };

        format!(
            "[{}] [{:5}] [{}] {}{}\n",
            timestamp, entry.level, entry.module, entry.message, context_str
        )
    }

    async fn rotate(&self, state: &mut FileSinkState) -> anyhow::Result<()> {
        state.file.shutdown().await?;

        for i in (1..self.max_files).rev() {
            let src = self.archive_path(i);
            let dst = self.archive_path(i + 1);
            if src.exists().await {
                tokio::fs::rename(&src, &dst).await?;
            }
        }

        let first_archive = self.archive_path(1);
        tokio::fs::rename(&self.base_path, &first_archive).await?;

        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.base_path)
            .await?;

        state.file = file;
        state.current_size = 0;
        Ok(())
    }

    fn archive_path(&self, index: usize) -> PathBuf {
        let mut path = self.base_path.clone();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "log".to_string());
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "log".to_string());
        path.set_file_name(format!("{}.{}.{}", stem, index, ext));
        path
    }
}

#[async_trait]
impl LogSink for FileSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        let line = self.format_entry(entry);
        let line_len = line.len() as u64;

        let mut state = self.state.lock().await;
        if state.current_size + line_len > self.max_size {
            self.rotate(&mut state).await?;
        }

        state.file.write_all(line.as_bytes()).await?;
        state.current_size += line_len;
        Ok(())
    }

    async fn flush(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        state.file.flush().await?;
        Ok(())
    }
}

trait PathExists {
    async fn exists(&self) -> bool;
}

impl PathExists for PathBuf {
    async fn exists(&self) -> bool {
        tokio::fs::try_exists(self).await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogLevel;
    use serde_json::json;
    use tempfile::tempdir;

    async fn setup() -> (FileSink, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let sink = FileSink::new(&path).await.unwrap();
        (sink, dir)
    }

    #[tokio::test]
    async fn test_write_entry() {
        let (sink, _dir) = setup().await;
        let entry = LogEntry::new(LogLevel::Info, "test", "hello", json!(null));
        sink.write(&entry).await.unwrap();
    }

    #[tokio::test]
    async fn test_writes_to_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");
        let sink = FileSink::new(&path).await.unwrap();

        let entry = LogEntry::new(LogLevel::Info, "mod", "msg", json!(null));
        sink.write(&entry).await.unwrap();
        sink.flush().await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("msg"));
        assert!(content.contains("[INFO]"));
    }

    #[tokio::test]
    async fn test_rotate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        // max_size = 10 bytes, triggers rotate on second write
        let sink = FileSink::with_config(&path, 10, 3).await.unwrap();

        let entry = LogEntry::new(
            LogLevel::Info,
            "mod",
            "hello world this is a long message",
            json!(null),
        );
        sink.write(&entry).await.unwrap();
        sink.write(&entry).await.unwrap();
        sink.flush().await.unwrap();

        let first = dir.path().join("test.1.log");
        let exists = first.exists().await;
        assert!(exists, "archive file should exist");
    }

    #[tokio::test]
    async fn test_flush() {
        let (sink, _dir) = setup().await;
        let entry = LogEntry::new(LogLevel::Info, "test", "flush check", json!(null));
        sink.write(&entry).await.unwrap();
        sink.flush().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.log");
        let sink = FileSink::new(&path).await.unwrap();

        for i in 0..10 {
            let entry = LogEntry::new(LogLevel::Debug, "loop", format!("entry {}", i), json!(null));
            sink.write(&entry).await.unwrap();
        }
        sink.flush().await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        for i in 0..10 {
            assert!(content.contains(&format!("entry {}", i)));
        }
    }
}
