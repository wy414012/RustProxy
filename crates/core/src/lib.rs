//! RustProxy Core — 共享核心库
//!
//! 提供配置解析、日志初始化、错误类型、TLS 证书管理、代理管理器等基础能力。

pub mod config;
pub mod error;
pub mod logger;
pub mod proxy_manager;
pub mod tls;

pub use error::Result;
