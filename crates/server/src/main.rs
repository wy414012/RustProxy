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

    // 设置流量统计回调
    let pm_for_traffic = app_state.proxy_manager();
    proxy_listener_mgr
        .set_traffic_stats_fn(Arc::new(
            move |proxy_name: String, bytes_in: u64, bytes_out: u64| {
                let pm = pm_for_traffic.clone();
                let name = proxy_name.clone();
                // 使用 tokio spawn 避免在同步回调中阻塞
                let rt = match tokio::runtime::Handle::try_current() {
                    Ok(h) => h,
                    Err(_) => return,
                };
                rt.spawn(async move {
                    pm.add_traffic(&name, bytes_in, bytes_out).await;
                });
            },
        ))
        .await;

    // 启动带宽采样周期任务（每 2 秒采样一次，计算实时带宽）
    let pm_for_bw = app_state.proxy_manager();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            pm_for_bw.update_bandwidth().await;
        }
    });

    // 为已有的代理规则启动公网监听器
    start_existing_listeners(&app_state, &proxy_listener_mgr, &config).await;

    // 启动 HTTP/HTTPS 共享监听器
    if config.server.http_port > 0 {
        if let Err(e) = proxy_listener_mgr
            .start_http_listener(config.server.bind_addr.clone(), config.server.http_port)
            .await
        {
            tracing::error!("启动 HTTP 代理监听失败: {}", e);
        }
    }

    if config.server.https_port > 0 {
        // HTTPS 使用独立的 TLS 配置（可以与隧道共享同一证书）
        let https_tls_config = cert::build_server_tls_config(&config.tls)?;
        if let Err(e) = proxy_listener_mgr
            .start_https_listener(
                config.server.bind_addr.clone(),
                config.server.https_port,
                https_tls_config,
            )
            .await
        {
            tracing::error!("启动 HTTPS 代理监听失败: {}", e);
        }
    }

    // 为已有的 HTTP/HTTPS 代理规则注册域名路由
    register_existing_domain_routes(&app_state, &proxy_listener_mgr).await;

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
                stop_proxy_listener(&mgr, &rule).await;
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
    if config.server.http_port > 0 {
        tracing::info!("HTTP 代理端口: {}", config.server.http_port);
    }
    if config.server.https_port > 0 {
        tracing::info!("HTTPS 代理端口: {}", config.server.https_port);
    }

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
        // 更新代理状态为 running
        app_state
            .proxy_manager()
            .update_status(
                &entry.rule.name,
                rustproxy_core::proxy_manager::ProxyStatus::Running,
            )
            .await;
        start_proxy_listener(listener_mgr, &entry.rule, config).await;
    }
}

/// 为已有的 HTTP/HTTPS 代理规则注册域名路由
async fn register_existing_domain_routes(
    app_state: &AppState,
    listener_mgr: &proxy_listener::ProxyListenerManager,
) {
    let proxies = app_state.proxy_manager().list_proxies().await;
    for entry in proxies {
        match entry.rule.proxy_type {
            rustproxy_core::config::ProxyType::Http | rustproxy_core::config::ProxyType::Https => {
                listener_mgr.register_domain_route(&entry.rule).await;
            }
            _ => {}
        }
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
            if rule.remote_port == 0 {
                tracing::warn!("TCP 代理 {} 未指定 remote_port", rule.name);
                return;
            }
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
            if rule.remote_port == 0 {
                tracing::warn!("UDP 代理 {} 未指定 remote_port", rule.name);
                return;
            }
            if let Err(e) = listener_mgr
                .start_udp_listener(
                    rule.name.clone(),
                    rule.remote_port,
                    config.server.bind_addr.clone(),
                    rule.client_id.clone(),
                )
                .await
            {
                tracing::error!("启动 UDP 代理监听 {} 失败: {}", rule.name, e);
            }
        }
        rustproxy_core::config::ProxyType::Http => {
            listener_mgr.register_domain_route(rule).await;
            tracing::info!(
                "HTTP 代理 {} 已注册域名路由: {:?}",
                rule.name,
                rule.custom_domains
            );
        }
        rustproxy_core::config::ProxyType::Https => {
            listener_mgr.register_domain_route(rule).await;
            tracing::info!(
                "HTTPS 代理 {} 已注册域名路由: {:?}",
                rule.name,
                rule.custom_domains
            );
        }
    }
}

/// 停止单个代理规则的公网监听器
async fn stop_proxy_listener(
    listener_mgr: &proxy_listener::ProxyListenerManager,
    rule: &rustproxy_core::config::ProxyRule,
) {
    match rule.proxy_type {
        rustproxy_core::config::ProxyType::Tcp | rustproxy_core::config::ProxyType::Udp => {
            listener_mgr.stop_listener(&rule.name).await;
        }
        rustproxy_core::config::ProxyType::Http | rustproxy_core::config::ProxyType::Https => {
            listener_mgr.unregister_domain_route(rule).await;
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
