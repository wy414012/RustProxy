//! TLS 隧道监听
//!
//! 监听客户端连接，区分主连接（Auth）和工作连接（NewWorkConnResp）。
//! 主连接走完整认证和消息循环；工作连接注册到 ProxyListenerManager 进行数据转发。

use std::sync::Arc;

use futures::StreamExt;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::Framed;

use rustproxy_core::config::ServerConfig as AppServerConfig;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{ControlMessage, Message};
use rustproxy_web::state::AppState;

use crate::client_session::{self, ClientSessionManager};
use crate::proxy_listener::ProxyListenerManager;

/// 启动隧道监听器
pub async fn run_tunnel_listener(
    config: &AppServerConfig,
    tls_config: Arc<ServerConfig>,
    state: AppState,
    session_manager: ClientSessionManager,
    proxy_listener_mgr: ProxyListenerManager,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.server.bind_addr, config.server.bind_port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("隧道监听: {}", addr);

    let tls_acceptor = TlsAcceptor::from(tls_config);
    let config = Arc::new(config.clone());

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::debug!("新连接: {}", peer_addr);

        let tls_acceptor = tls_acceptor.clone();
        let config = config.clone();
        let state = state.clone();
        let session_manager = session_manager.clone();
        let proxy_listener_mgr = proxy_listener_mgr.clone();

        tokio::spawn(async move {
            match tls_acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    tracing::debug!("TLS 握手成功: {}", peer_addr);
                    let framed = Framed::new(tls_stream, FrameCodec);

                    // 先读取第一条消息，判断连接类型
                    dispatch_connection(framed, config, state, session_manager, proxy_listener_mgr)
                        .await;
                }
                Err(e) => {
                    tracing::warn!("TLS 握手失败 {}: {}", peer_addr, e);
                }
            }
        });
    }
}

/// 根据第一条消息判断连接类型并分发处理
async fn dispatch_connection(
    mut framed: Framed<tokio_rustls::server::TlsStream<tokio::net::TcpStream>, FrameCodec>,
    config: Arc<AppServerConfig>,
    state: AppState,
    session_manager: ClientSessionManager,
    proxy_listener_mgr: ProxyListenerManager,
) {
    // 读取第一条消息，10 秒超时
    let first_msg =
        match tokio::time::timeout(std::time::Duration::from_secs(10), framed.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                tracing::warn!("首消息解析错误: {}", e);
                return;
            }
            Ok(None) => {
                tracing::debug!("连接在首消息前关闭");
                return;
            }
            Err(_) => {
                tracing::warn!("首消息超时");
                return;
            }
        };

    match first_msg {
        // 主连接：Auth 消息 → 走完整客户端会话流程
        Message::Control(ControlMessage::Auth(auth_req)) => {
            tracing::debug!("主连接: client_id={}", auth_req.client_id);
            client_session::handle_client_with_auth(
                framed,
                auth_req,
                config,
                state,
                session_manager,
            )
            .await;
        }
        // 工作连接：NewWorkConnResp → 注册到代理监听管理器
        Message::Control(ControlMessage::NewWorkConnResp(resp)) => {
            tracing::debug!(
                "工作连接: proxy={}, conn_id={}, success={}",
                resp.proxy_name,
                resp.conn_id,
                resp.success
            );
            if resp.success {
                proxy_listener_mgr
                    .register_work_conn(resp.conn_id, framed)
                    .await;
            } else {
                tracing::warn!(
                    "工作连接建立失败: proxy={}, conn_id={}",
                    resp.proxy_name,
                    resp.conn_id
                );
            }
        }
        other => {
            tracing::warn!("非预期首消息类型: {:?}", other);
        }
    }
}
