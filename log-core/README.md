# log-core

[![Crates.io](https://img.shields.io/crates/v/log-core.svg)](https://crates.io/crates/log-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Unified logging foundation. Provides a `LogSink` trait + `Logger` combinator with support for terminal, file, and cloud backends.

## Usage

```rust
use log_core::{Logger, LogLevel, ConsoleSink, FileSink};
use serde_json::json;

// Terminal + File
let logger = Logger::builder()
    .console()
    .file(FileSink::new("ops.log").await?)
    .min_level(LogLevel::Info)
    .build();

// Cloud upload: configure LOG_CLOUD_URL + LOG_CLOUD_KEY in .env
// Logger::builder().console().file("ops.log").cloud().build();

logger.info("mod", "message", json!({"key": "value"}));
logger.warn("mod", "warning", json!({"latency_ms": 2500}));
logger.error("mod", "error", json!({"code": 500}));
```

## Sinks

| Sink | Use Case |
|------|----------|
| `ConsoleSink` | Colorized stderr output |
| `FileSink` | Async file writing with auto-rotation (10 MB) |
| `CloudSink` | Cloud upload (phase 2, auto-POST at >=Warn) |
| Custom | Implement the `LogSink` trait |

## Tracing Integration

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

[中文文档](README_CN.md)
