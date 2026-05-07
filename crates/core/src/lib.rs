//! RustProxy Core — 共享核心库
//!
//! 提供配置解析、日志初始化、错误类型、工具函数等基础能力。

pub mod config;
pub mod error;
pub mod logger;

pub use error::Result;
