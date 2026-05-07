//! RustProxy Server — 服务端入口

mod cert;
mod client_session;
mod tunnel;

use anyhow::Result;
use clap::Parser;
use rustproxy_core::logger;
use rustproxy_web::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "rustproxy-server", version = VERSION, about = "RustProxy 服务端")]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "server.toml")]
    config: String,

    /// 数据库文件路径（默认与配置文件同目录下的 rustproxy.db）
    #[arg(long)]
    db: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    logger::init();

    let args = Args::parse();
    tracing::info!("RustProxy Server v{}", VERSION);

    // 加载配置
    let config_content = std::fs::read_to_string(&args.config)
        .map_err(|e| anyhow::anyhow!("读取配置文件失败 {}: {}", args.config, e))?;
    let config = rustproxy_core::config::parse_server_config(&config_content)?;
    tracing::info!("配置已加载: {}", args.config);

    // 确定数据库路径
    let db_path = args.db.clone().unwrap_or_else(|| {
        // 默认与配置文件同目录
        let config_dir = std::path::Path::new(&args.config)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        config_dir
            .join("rustproxy.db")
            .to_string_lossy()
            .to_string()
    });
    tracing::info!("数据库路径: {}", db_path);

    // 初始化 TLS 证书
    let tls_config = cert::build_server_tls_config(&config.tls)?;
    tracing::info!("TLS 证书已就绪");

    // 初始化共享状态（带数据库）
    let app_state = AppState::with_db(config.clone(), &db_path)?;

    // 从数据库加载已有代理规则到内存运行时状态
    app_state.proxy_manager().load_from_db().await;

    // 启动隧道监听
    let tunnel_state = app_state.clone();
    let tunnel_config = config.clone();
    let tunnel_tls = tls_config.clone();
    let tunnel_handle = tokio::spawn(async move {
        if let Err(e) = tunnel::run_tunnel_listener(&tunnel_config, tunnel_tls, tunnel_state).await
        {
            tracing::error!("隧道监听异常退出: {}", e);
        }
    });

    // 启动 Web 管理面板
    if config.web.enable {
        let web_state = app_state.clone();
        let web_config = config.web.clone();
        tokio::spawn(async move {
            if let Err(e) = run_web_panel(&web_config, web_state).await {
                tracing::error!("Web 面板异常退出: {}", e);
            }
        });
        tracing::info!(
            "Web 管理面板: http://{}:{}",
            config.web.bind_addr,
            config.web.bind_port
        );
    }

    tracing::info!("服务端已启动，隧道端口: {}", config.server.bind_port);

    // 等待退出信号
    tokio::signal::ctrl_c().await?;
    tracing::info!("正在关闭服务端...");

    tunnel_handle.abort();

    tracing::info!("服务端已关闭");
    Ok(())
}

async fn run_web_panel(
    web_config: &rustproxy_core::config::WebSection,
    state: AppState,
) -> Result<()> {
    let app = rustproxy_web::build_app(state);
    let addr = format!("{}:{}", web_config.bind_addr, web_config.bind_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Web 面板监听: {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
