//! RustProxy Server — 服务端入口

use anyhow::Result;
use rustproxy_core::logger;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();

    tracing::info!("RustProxy Server v{}", VERSION);

    // TODO: 解析命令行参数，加载配置
    // TODO: 初始化 TLS 证书
    // TODO: 启动隧道监听
    // TODO: 启动代理管理器
    // TODO: 启动 Web 管理面板

    tracing::info!("Server started");

    // 临时保持运行
    tokio::signal::ctrl_c().await?;
    tracing::info!("Server shutting down...");

    Ok(())
}
