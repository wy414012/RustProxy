//! 客户端会话管理
//!
//! 管理已连接的客户端：认证、消息收发、心跳检测。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// 处理已认证的客户端连接（从 tunnel.rs 分发过来，Auth 已读取）
pub async fn handle_client_with_auth(
    mut framed: Framed<TlsStream<TcpStream>, FrameCodec>,
    auth_req: AuthRequest,
    config: Arc<ServerConfig>,
    app_state: AppState,
    session_manager: ClientSessionManager,
) {
    // 1. 验证 client_id 格式
    if !validate_client_id(&auth_req.client_id) {
        tracing::warn!("客户端 ID 格式无效: {}", auth_req.client_id);
        let resp = AuthResponse {
            success: false,
            message: "客户端 ID 格式无效".to_string(),
            server_version: String::new(),
        };
        let _ = framed
            .send(Message::Control(ControlMessage::AuthResp(resp)))
            .await;
        return;
    }

    // 2. 验证 Token（常量时间比较，防止时序攻击）
    if !constant_time_token_eq(&auth_req.token, &config.server.token) {
        tracing::warn!("客户端 {} Token 验证失败", auth_req.client_id);
        let resp = AuthResponse {
            success: false,
            message: "Token 验证失败".to_string(),
            server_version: String::new(), // 认证失败不暴露版本信息
        };
        let _ = framed
            .send(Message::Control(ControlMessage::AuthResp(resp)))
            .await;
        return;
    }

    let client_id = auth_req.client_id.clone();

    // 3. 发送认证成功响应
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
        return;
    }

    tracing::info!("客户端 {} (v{}) 认证成功", client_id, auth_req.version);

    // 4. 创建消息通道
    let (tx, mut rx) = mpsc::channel::<Message>(256);

    // 5. 注册会话
    session_manager.register(client_id.clone(), tx).await;

    // 更新 AppState 中的客户端列表
    let clients = session_manager.connected_clients().await;
    app_state.set_connected_clients(clients).await;

    // 6. 推送该客户端的所有代理规则
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
            proxy_protocol: entry.rule.proxy_protocol.clone(),
        };
        let msg = Message::Control(ControlMessage::ServerAssignProxy(assign_req));
        if framed.send(msg).await.is_err() {
            break;
        }
        tracing::debug!("推送代理规则 {} 到客户端 {}", entry.rule.name, client_id);
    }

    // 7. 心跳定时器
    let heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    tokio::pin!(heartbeat_interval);
    let mut last_heartbeat = Instant::now();
    const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90); // 3 倍心跳间隔

    // 8. 进入消息循环
    loop {
        tokio::select! {
            // 接收客户端消息
            result = framed.next() => {
                match result {
                    Some(Ok(Message::Control(ControlMessage::Ping))) => {
                        last_heartbeat = Instant::now();
                        if framed.send(Message::Control(ControlMessage::Pong)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Control(ControlMessage::Auth(_)))) => {
                        // 忽略重复认证
                        tracing::debug!("客户端 {} 发送重复认证消息，忽略", client_id);
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
            // 心跳检测
            _ = heartbeat_interval.tick() => {
                if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
                    tracing::warn!("客户端 {} 心跳超时，断开连接", client_id);
                    break;
                }
            }
        }
    }

    // 9. 清理
    session_manager.unregister(&client_id).await;
    let clients = session_manager.connected_clients().await;
    app_state.set_connected_clients(clients).await;
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 验证客户端 ID 格式（仅允许字母、数字、横线、下划线、点号，1-64字符）
fn validate_client_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    id.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// 常量时间 Token 比较，防止时序攻击
fn constant_time_token_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a_bytes.iter().zip(b_bytes.iter()) {
        result |= x ^ y;
    }
    result == 0
}
