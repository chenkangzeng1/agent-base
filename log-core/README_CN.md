# log-core

[![Crates.io](https://img.shields.io/crates/v/log-core.svg)](https://crates.io/crates/log-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

统一日志底座。提供 `LogSink` trait + `Logger` 组合器，支持终端 / 文件 / 云端等多种输出目标。

## 用法

```rust
use log_core::{Logger, LogLevel, ConsoleSink, FileSink};
use serde_json::json;

// 终端 + 文件
let logger = Logger::builder()
    .console()
    .file(FileSink::new("ops.log").await?)
    .min_level(LogLevel::Info)
    .build();

// 云端上报：.env 配置 LOG_CLOUD_URL + LOG_CLOUD_KEY
// Logger::builder().console().file("ops.log").cloud().build();

logger.info("mod", "message", json!({"key": "value"}));
logger.warn("mod", "warning", json!({"latency_ms": 2500}));
logger.error("mod", "error", json!({"code": 500}));
```

## Sink

| Sink | 用途 |
|------|------|
| `ConsoleSink` | 带颜色 stderr 输出 |
| `FileSink` | 异步文件写入，超 10MB 自动滚动 |
| `CloudSink` | 云端上报（二期，>=Warn 自动 POST） |
| 自定义 | 实现 `LogSink` trait 即可 |

## Tracing 集成

```rust
use log_core::LogCoreLayer;
use tracing_subscriber::prelude::*;

let layer = LogCoreLayer::file("app.log", log_core::LogLevel::Info).await?;

tracing_subscriber::registry()
    .with(tracing_subscriber::EnvFilter::new("info"))
    .with(layer)
    .init();
```

## License

MIT
