//! RustProxy Client — 客户端入口

use anyhow::Result;
use rustproxy_core::logger;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();

    tracing::info!("RustProxy Client v{}", VERSION);

    // TODO: 解析命令行参数，加载配置
    // TODO: 连接服务端隧道
    // TODO: 认证
    // TODO: 注册代理规则
    // TODO: 启动本地代理工作协程

    tracing::info!("Client started");

    // 临时保持运行
    tokio::signal::ctrl_c().await?;
    tracing::info!("Client shutting down...");

    Ok(())
}
