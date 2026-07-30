use std::sync::Arc;

use async_trait::async_trait;
use log_core::{LogEntry, LogLevel, LogSink, Logger};
use serde_json::json;
use tokio::sync::Mutex;

/// 模拟云上报的 sink —— 收集 >= Warn 的日志
struct MockCloudSink {
    uploaded: Arc<Mutex<Vec<LogEntry>>>,
}

#[async_trait]
impl LogSink for MockCloudSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        if entry.level >= LogLevel::Warn {
            self.uploaded.lock().await.push(entry.clone());
        }
        Ok(())
    }
}

/// 模拟本地文件 sink —— 收集全部日志
struct MockFileSink {
    entries: Arc<Mutex<Vec<LogEntry>>>,
}

#[async_trait]
impl LogSink for MockFileSink {
    async fn write(&self, entry: &LogEntry) -> anyhow::Result<()> {
        self.entries.lock().await.push(entry.clone());
        Ok(())
    }
}

#[tokio::test]
async fn test_realistic_logger_usage() {
    // 模拟：产品环境的 Logger = 本地文件 + 云端上报（>=Warn）
    let uploaded = Arc::new(Mutex::new(Vec::new()));
    let file_entries = Arc::new(Mutex::new(Vec::new()));

    let cloud = MockCloudSink {
        uploaded: uploaded.clone(),
    };
    let file = MockFileSink {
        entries: file_entries.clone(),
    };

    let logger = Logger::builder()
        .sink(Box::new(file))
        .sink(Box::new(cloud))
        .min_level(LogLevel::Info)
        .build();

    // ------ 模拟正常运行 ------
    logger
        .info(
            "agent::session",
            "session created",
            json!({"session_id": 1}),
        )
        .await;
    logger
        .info(
            "agent::ssh",
            "connected to server",
            json!({"host": "192.168.1.1", "latency_ms": 15}),
        )
        .await;

    // ------ 模拟警告 ------
    logger
        .warn(
            "agent::ssh",
            "connection slow",
            json!({"host": "192.168.1.1", "latency_ms": 2500}),
        )
        .await;

    // ------ 模拟错误 ------
    logger
        .error(
            "agent::plan",
            "step execution failed",
            json!({"step": 3, "error": "permission denied", "command": "systemctl restart nginx"}),
        )
        .await;

    // ------ Debug 被 min_level 过滤掉 ------
    logger
        .debug(
            "agent::internal",
            "debug info",
            json!({"detail": "should not appear"}),
        )
        .await;

    // 验证所有日志都到了本地文件
    {
        let entries = file_entries.lock().await;
        assert_eq!(entries.len(), 4, "file should have all 4 entries");
        assert_eq!(entries[0].module, "agent::session");
        assert_eq!(entries[1].level, LogLevel::Info);
        assert_eq!(entries[2].level, LogLevel::Warn);
        assert_eq!(entries[3].level, LogLevel::Error);
    }

    // 验证只有 Warn + Error 上传到了云端
    {
        let up = uploaded.lock().await;
        assert_eq!(up.len(), 2, "cloud should only have warn+error");
        assert_eq!(up[0].level, LogLevel::Warn);
        assert_eq!(up[0].message, "connection slow");
        assert_eq!(up[1].level, LogLevel::Error);
        assert_eq!(up[1].message, "step execution failed");
    }
}
