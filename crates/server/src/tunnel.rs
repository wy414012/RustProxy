//! TLS 隧道监听
//!
//! 监听客户端连接，为每个客户端创建会话。

use std::sync::Arc;

use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use rustproxy_core::config::ServerConfig as AppServerConfig;
use rustproxy_web::state::AppState;

use crate::client_session::{self, ClientSessionManager};

/// 启动隧道监听器
pub async fn run_tunnel_listener(
    config: &AppServerConfig,
    tls_config: Arc<ServerConfig>,
    state: AppState,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.server.bind_addr, config.server.bind_port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("隧道监听: {}", addr);

    let tls_acceptor = TlsAcceptor::from(tls_config);
    let session_manager = ClientSessionManager::new();

    let config = Arc::new(config.clone());

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::debug!("新连接: {}", peer_addr);

        let tls_acceptor = tls_acceptor.clone();
        let config = config.clone();
        let state = state.clone();
        let session_manager = session_manager.clone();

        tokio::spawn(async move {
            match tls_acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    tracing::debug!("TLS 握手成功: {}", peer_addr);
                    client_session::handle_client(tls_stream, config, state, session_manager).await;
                }
                Err(e) => {
                    tracing::warn!("TLS 握手失败 {}: {}", peer_addr, e);
                }
            }
        });
    }
}
