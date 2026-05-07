//! RustProxy Server — 服务端入口

mod cert;
mod client_session;
mod proxy_listener;
mod tunnel;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use rustproxy_core::config::ProxyRule;
use rustproxy_core::logger;
use rustproxy_proto::message::{ControlMessage, Message};
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

    // 初始化客户端会话管理器
    let session_manager = client_session::ClientSessionManager::new();

    // 初始化代理监听管理器
    let proxy_listener_mgr = proxy_listener::ProxyListenerManager::new(session_manager.clone());

    // 为已有的 TCP 代理规则启动公网监听器
    start_existing_listeners(&app_state, &proxy_listener_mgr, &config).await;

    // --- 设置回调（注意 clone 顺序，避免 move 问题） ---

    // 1. 设置通知客户端回调
    let sm_for_notify = session_manager.clone();
    app_state
        .set_notify_client(Arc::new(move |cid: String, msg_json: String| {
            let sm = sm_for_notify.clone();
            let rt = match tokio::runtime::Handle::try_current() {
                Ok(h) => h,
                Err(_) => return false,
            };
            rt.block_on(async {
                if let Ok(msg) = serde_json::from_str::<ControlMessage>(&msg_json) {
                    sm.send_to(&cid, Message::Control(msg)).await
                } else {
                    false
                }
            })
        }))
        .await;

    // 2. 设置代理规则创建回调
    let mgr_for_create = proxy_listener_mgr.clone();
    let config_for_create = config.clone();
    app_state
        .set_on_proxy_create(Arc::new(move |rule: ProxyRule| {
            let mgr = mgr_for_create.clone();
            let config = config_for_create.clone();
            let rt = match tokio::runtime::Handle::try_current() {
                Ok(h) => h,
                Err(_) => return,
            };
            rt.spawn(async move {
                start_proxy_listener(&mgr, &rule, &config).await;
            });
        }))
        .await;

    // 3. 设置代理规则删除回调
    let mgr_for_delete = proxy_listener_mgr.clone();
    app_state
        .set_on_proxy_delete(Arc::new(move |rule: ProxyRule| {
            let mgr = mgr_for_delete.clone();
            let rt = match tokio::runtime::Handle::try_current() {
                Ok(h) => h,
                Err(_) => return,
            };
            rt.spawn(async move {
                mgr.stop_listener(&rule.name).await;
            });
        }))
        .await;

    // --- 启动服务 ---

    // 启动隧道监听
    let tunnel_state = app_state.clone();
    let tunnel_config = config.clone();
    let tunnel_tls = tls_config.clone();
    let tunnel_session = session_manager.clone();
    let tunnel_listener_mgr = proxy_listener_mgr.clone();
    let tunnel_handle = tokio::spawn(async move {
        if let Err(e) = tunnel::run_tunnel_listener(
            &tunnel_config,
            tunnel_tls,
            tunnel_state,
            tunnel_session,
            tunnel_listener_mgr,
        )
        .await
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
    proxy_listener_mgr.stop_all().await;

    tracing::info!("服务端已关闭");
    Ok(())
}

/// 为已有的代理规则启动公网监听器
async fn start_existing_listeners(
    app_state: &AppState,
    listener_mgr: &proxy_listener::ProxyListenerManager,
    config: &rustproxy_core::config::ServerConfig,
) {
    let proxies = app_state.proxy_manager().list_proxies().await;
    for entry in proxies {
        start_proxy_listener(listener_mgr, &entry.rule, config).await;
    }
}

/// 启动单个代理规则的公网监听器
async fn start_proxy_listener(
    listener_mgr: &proxy_listener::ProxyListenerManager,
    rule: &rustproxy_core::config::ProxyRule,
    config: &rustproxy_core::config::ServerConfig,
) {
    match rule.proxy_type {
        rustproxy_core::config::ProxyType::Tcp => {
            if let Err(e) = listener_mgr
                .start_tcp_listener(
                    rule.name.clone(),
                    rule.remote_port,
                    config.server.bind_addr.clone(),
                    rule.client_id.clone(),
                )
                .await
            {
                tracing::error!("启动 TCP 代理监听 {} 失败: {}", rule.name, e);
            }
        }
        rustproxy_core::config::ProxyType::Udp => {
            tracing::info!("UDP 代理 {} 暂未实现公网监听", rule.name);
        }
        rustproxy_core::config::ProxyType::Http | rustproxy_core::config::ProxyType::Https => {
            tracing::info!(
                "{} 代理 {} 使用虚拟主机路由，无需独立监听端口",
                rule.proxy_type,
                rule.name
            );
        }
    }
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
