//! 错误类型定义

use thiserror::Error;

/// RustProxy 核心错误类型
#[derive(Error, Debug)]
pub enum Error {
    #[error("配置解析错误: {0}")]
    Config(#[from] toml::de::Error),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("TLS 错误: {0}")]
    Tls(String),

    #[error("隧道错误: {0}")]
    Tunnel(String),

    #[error("代理错误: {0}")]
    Proxy(String),

    #[error("认证失败: {0}")]
    Auth(String),
}

/// 统一 Result 类型
pub type Result<T> = std::result::Result<T, Error>;
