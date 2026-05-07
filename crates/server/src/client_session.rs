//! 客户端会话管理
//!
//! 管理已连接的客户端：认证、消息收发、心跳检测。

use std::collections::HashMap;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_rustls::server::TlsStream;
use tokio_util::codec::Framed;

use rustproxy_core::config::ServerConfig;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{
    AuthRequest, AuthResponse, ControlMessage, Message, ServerAssignProxyRequest,
};
use rustproxy_web::state::AppState;

/// 客户端会话
struct ClientSession {
    client_id: String,
    tx: mpsc::Sender<Message>,
}

impl std::fmt::Debug for ClientSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSession")
            .field("client_id", &self.client_id)
            .finish()
    }
}

/// 客户端会话管理器
#[derive(Clone)]
pub struct ClientSessionManager {
    inner: Arc<RwLock<HashMap<String, ClientSession>>>,
}

impl std::fmt::Debug for ClientSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSessionManager").finish()
    }
}

impl ClientSessionManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 注册客户端会话
    pub async fn register(&self, client_id: String, tx: mpsc::Sender<Message>) {
        let mut inner = self.inner.write().await;
        if let Some(old) = inner.remove(&client_id) {
            tracing::warn!("客户端 {} 旧连接被新连接替换", client_id);
            let _ = old.tx.send(Message::Control(ControlMessage::Ping)).await;
        }
        inner.insert(client_id.clone(), ClientSession { client_id, tx });
    }

    /// 注销客户端会话
    pub async fn unregister(&self, client_id: &str) {
        let mut inner = self.inner.write().await;
        inner.remove(client_id);
        tracing::info!("客户端 {} 已断开", client_id);
    }

    /// 向指定客户端发送消息
    pub async fn send_to(&self, client_id: &str, msg: Message) -> bool {
        let inner = self.inner.read().await;
        if let Some(session) = inner.get(client_id) {
            session.tx.send(msg).await.is_ok()
        } else {
            false
        }
    }

    /// 获取已连接的客户端 ID 列表
    pub async fn connected_clients(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner.keys().cloned().collect()
    }

    /// 检查客户端是否在线
    pub async fn _is_online(&self, client_id: &str) -> bool {
        let inner = self.inner.read().await;
        inner.contains_key(client_id)
    }
}

impl Default for ClientSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 处理单个客户端连接
pub async fn handle_client(
    stream: TlsStream<TcpStream>,
    config: Arc<ServerConfig>,
    app_state: AppState,
    session_manager: ClientSessionManager,
) {
    let mut framed = Framed::new(stream, FrameCodec);

    // 1. 认证阶段
    let client_id = match wait_for_auth(&mut framed, &config).await {
        Some(id) => id,
        None => return,
    };

    // 2. 创建消息通道
    let (tx, mut rx) = mpsc::channel::<Message>(256);

    // 3. 注册会话
    session_manager.register(client_id.clone(), tx).await;
    tracing::info!("客户端 {} 认证成功", client_id);

    // 更新 AppState 中的客户端列表
    let clients = session_manager.connected_clients().await;
    app_state.set_connected_clients(clients).await;

    // 设置通知回调
    let sm = session_manager.clone();
    app_state
        .set_notify_client(Arc::new(move |cid: &str, msg_json: &str| {
            let sm = sm.clone();
            let cid = cid.to_string();
            let msg_json = msg_json.to_string();
            // 同步回调，用 try_send 避免阻塞
            // 这里简化为直接通过 tokio spawn
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                if let Ok(msg) = serde_json::from_str::<ControlMessage>(&msg_json) {
                    sm.send_to(&cid, Message::Control(msg)).await
                } else {
                    false
                }
            })
        }))
        .await;

    // 4. 推送该客户端的所有代理规则
    let proxy_manager = app_state.proxy_manager();
    let proxies = proxy_manager.list_proxies_by_client(&client_id).await;
    for entry in proxies {
        let assign_req = ServerAssignProxyRequest {
            name: entry.rule.name.clone(),
            proxy_type: entry.rule.proxy_type.to_string(),
            local_ip: entry.rule.local_ip.clone(),
            local_port: entry.rule.local_port,
            remote_port: entry.rule.remote_port,
            custom_domains: entry.rule.custom_domains.clone(),
        };
        let msg = Message::Control(ControlMessage::ServerAssignProxy(assign_req));
        if framed.send(msg).await.is_err() {
            break;
        }
        tracing::debug!("推送代理规则 {} 到客户端 {}", entry.rule.name, client_id);
    }

    // 5. 心跳定时器
    let heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    tokio::pin!(heartbeat_interval);

    // 6. 进入消息循环
    loop {
        tokio::select! {
            // 接收客户端消息
            result = framed.next() => {
                match result {
                    Some(Ok(Message::Control(ControlMessage::Ping))) => {
                        if framed.send(Message::Control(ControlMessage::Pong)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // 其他消息后续处理
                    }
                    Some(Err(e)) => {
                        tracing::warn!("客户端 {} 消息解析错误: {}", client_id, e);
                        break;
                    }
                    None => {
                        tracing::info!("客户端 {} 连接关闭", client_id);
                        break;
                    }
                }
            }
            // 接收服务端推送
            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        if framed.send(msg).await.is_err() {
                            tracing::warn!("发送消息到客户端 {} 失败", client_id);
                            break;
                        }
                    }
                    None => break,
                }
            }
            // 心跳超时检测（暂不实现严格超时）
            _ = heartbeat_interval.tick() => {
                // 检查客户端是否长时间无心跳
            }
        }
    }

    // 7. 清理
    session_manager.unregister(&client_id).await;
    let clients = session_manager.connected_clients().await;
    app_state.set_connected_clients(clients).await;
}

/// 等待客户端认证
async fn wait_for_auth(
    framed: &mut Framed<TlsStream<TcpStream>, FrameCodec>,
    config: &ServerConfig,
) -> Option<String> {
    use tokio::time::{timeout, Duration};

    match timeout(Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(Message::Control(ControlMessage::Auth(AuthRequest {
            client_id,
            token,
            version,
        }))))) => {
            if token != config.server.token {
                tracing::warn!("客户端 {} Token 验证失败", client_id);
                let resp = AuthResponse {
                    success: false,
                    message: "Token 验证失败".to_string(),
                    server_version: VERSION.to_string(),
                };
                let _ = framed
                    .send(Message::Control(ControlMessage::AuthResp(resp)))
                    .await;
                return None;
            }

            let resp = AuthResponse {
                success: true,
                message: "认证成功".to_string(),
                server_version: VERSION.to_string(),
            };
            if framed
                .send(Message::Control(ControlMessage::AuthResp(resp)))
                .await
                .is_err()
            {
                return None;
            }
            tracing::info!("客户端 {} (v{}) 认证成功", client_id, version);
            Some(client_id)
        }
        Ok(Some(Ok(_))) => {
            tracing::warn!("期望认证消息，收到其他消息");
            None
        }
        Ok(Some(Err(e))) => {
            tracing::warn!("认证阶段消息解析错误: {}", e);
            None
        }
        Ok(None) => {
            tracing::warn!("客户端在认证前断开连接");
            None
        }
        Err(_) => {
            tracing::warn!("认证超时（10s）");
            None
        }
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");
