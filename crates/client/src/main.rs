//! RustProxy Client — 客户端入口
//!
//! 客户端只需配置服务端地址、端口、Token 和客户端 ID。
//! 代理规则由服务端通过 Web 面板管理，实时推送到客户端。
//! 客户端每次启动从服务端拉取规则，无需本地持久化。

mod connector;
mod proxy_worker;

use anyhow::Result;
use clap::Parser;
use rustproxy_core::logger;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "rustproxy-client", version = VERSION, about = "RustProxy 客户端")]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "client.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();

    let args = Args::parse();
    tracing::info!("RustProxy Client v{}", VERSION);

    // 加载配置
    let config_content = std::fs::read_to_string(&args.config)
        .map_err(|e| anyhow::anyhow!("读取配置文件失败 {}: {}", args.config, e))?;
    let config = rustproxy_core::config::parse_client_config(&config_content)?;
    tracing::info!(
        "配置已加载: 客户端 ID={}, 服务端={}:{}",
        config.client.id,
        config.client.server_addr,
        config.client.server_port
    );

    // 带重连的主循环
    let mut retry_delay = std::time::Duration::from_secs(1);
    let max_delay = std::time::Duration::from_secs(60);

    loop {
        match connector::connect_and_run(&config).await {
            Ok(()) => {
                tracing::info!("连接正常关闭");
                retry_delay = std::time::Duration::from_secs(1);
            }
            Err(e) => {
                tracing::warn!("连接断开: {}, {} 后重连", e, retry_delay.as_secs());
            }
        }

        tokio::time::sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(max_delay);
    }
}
