//! 服务端公网监听器
//!
//! 为每个 TCP 代理规则在服务端创建公网端口监听器。
//! 外部用户连接到公网端口后，服务端通知客户端建立工作连接，
//! 然后在两条连接之间桥接双向数据转发。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{ControlMessage, DataMessage, Message, NewWorkConnRequest};

use crate::client_session::ClientSessionManager;

/// 连接 ID 计数器（全局递增）
static CONN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 代理监听管理器
#[derive(Clone)]
pub struct ProxyListenerManager {
    inner: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
    session_manager: ClientSessionManager,
    /// 工作连接等待队列：conn_id -> 等待发送端的 channel
    pending_work_conns: Arc<RwLock<HashMap<u64, tokio::sync::oneshot::Sender<FramedProxyConn>>>>,
}

/// Framed 代理连接（用于双向数据转发）
type FramedProxyConn = Framed<tokio_rustls::server::TlsStream<tokio::net::TcpStream>, FrameCodec>;

impl std::fmt::Debug for ProxyListenerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyListenerManager").finish()
    }
}

impl ProxyListenerManager {
    pub fn new(session_manager: ClientSessionManager) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            session_manager,
            pending_work_conns: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动 TCP 代理监听器
    pub async fn start_tcp_listener(
        &self,
        name: String,
        remote_port: u16,
        bind_addr: String,
        client_id: String,
    ) -> anyhow::Result<()> {
        let addr = format!("{}:{}", bind_addr, remote_port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("TCP 代理监听已启动: {} -> 端口 {}", name, remote_port);

        let name_for_insert = name.clone();
        let mgr = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        tracing::debug!("代理 {} 收到外部连接: {}", name, peer_addr);
                        let mgr = mgr.clone();
                        let proxy_name = name.clone();
                        let cid = client_id.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                mgr.handle_tcp_proxy_conn(&proxy_name, &cid, stream).await
                            {
                                tracing::warn!("代理 {} 处理连接失败: {}", proxy_name, e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("代理 {} 接受连接失败: {}", name, e);
                    }
                }
            }
        });

        let mut inner = self.inner.write().await;
        inner.insert(name_for_insert, handle);
        Ok(())
    }

    /// 停止代理监听器
    pub async fn stop_listener(&self, name: &str) {
        let mut inner = self.inner.write().await;
        if let Some(handle) = inner.remove(name) {
            handle.abort();
            tracing::info!("代理监听 {} 已停止", name);
        }
    }

    /// 停止所有代理监听器
    pub async fn stop_all(&self) {
        let mut inner = self.inner.write().await;
        for (name, handle) in inner.drain() {
            handle.abort();
            tracing::debug!("代理监听 {} 已停止", name);
        }
    }

    /// 注册工作连接（客户端发起的工作 TLS 连接）
    pub async fn register_work_conn(&self, conn_id: u64, framed: FramedProxyConn) -> bool {
        let mut pending = self.pending_work_conns.write().await;
        if let Some(sender) = pending.remove(&conn_id) {
            // 有等待的代理连接，发送过去
            if sender.send(framed).is_ok() {
                tracing::debug!("工作连接 {} 已匹配到等待的代理", conn_id);
                return true;
            }
        }
        tracing::warn!("工作连接 {} 没有匹配的等待代理", conn_id);
        false
    }

    /// 处理 TCP 代理的新外部连接
    async fn handle_tcp_proxy_conn(
        &self,
        proxy_name: &str,
        client_id: &str,
        user_stream: TcpStream,
    ) -> anyhow::Result<()> {
        // 1. 分配连接 ID
        let conn_id = CONN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

        // 2. 创建 oneshot 通道等待工作连接
        let (tx, rx) = tokio::sync::oneshot::channel::<FramedProxyConn>();

        // 3. 注册等待队列
        {
            let mut pending = self.pending_work_conns.write().await;
            pending.insert(conn_id, tx);
        }

        // 4. 通知客户端建立工作连接
        let work_req = NewWorkConnRequest {
            proxy_name: proxy_name.to_string(),
            conn_id,
        };
        let sent = self
            .session_manager
            .send_to(
                client_id,
                Message::Control(ControlMessage::NewWorkConn(work_req)),
            )
            .await;

        if !sent {
            // 客户端不在线，清理
            let mut pending = self.pending_work_conns.write().await;
            pending.remove(&conn_id);
            anyhow::bail!("客户端 {} 不在线，无法建立工作连接", client_id);
        }

        // 5. 等待客户端建立工作连接（30 秒超时）
        let work_framed = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(framed)) => framed,
            Ok(Err(_)) => {
                let mut pending = self.pending_work_conns.write().await;
                pending.remove(&conn_id);
                anyhow::bail!("工作连接通道关闭");
            }
            Err(_) => {
                let mut pending = self.pending_work_conns.write().await;
                pending.remove(&conn_id);
                anyhow::bail!("等待工作连接超时");
            }
        };

        tracing::info!("代理 {} 连接 {} 已桥接", proxy_name, conn_id);

        // 6. 双向数据转发：用户 TCP ↔ 工作 TLS
        bridge_tcp_to_framed(user_stream, work_framed, conn_id).await;

        tracing::info!("代理 {} 连接 {} 已结束", proxy_name, conn_id);
        Ok(())
    }
}

/// 双向转发：TCP 用户连接 ↔ Framed TLS 工作连接
///
/// 用户数据通过 DataMessage 封装发送到工作连接，
/// 工作连接的 DataMessage 解包后写回用户 TCP。
async fn bridge_tcp_to_framed(
    mut user_tcp: TcpStream,
    mut work_framed: FramedProxyConn,
    conn_id: u64,
) {
    let mut tcp_buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            // 从用户 TCP 读取数据，通过 Framed 发送到客户端
            result = user_tcp.readable() => {
                if result.is_err() {
                    break;
                }
                match user_tcp.try_read(&mut tcp_buf) {
                    Ok(0) => {
                        // 用户关闭连接
                        tracing::debug!("连接 {} 用户关闭", conn_id);
                        let _ = work_framed.send(Message::Data(DataMessage {
                            conn_id,
                            data: bytes::Bytes::new(),
                        })).await;
                        let _ = work_framed.close().await;
                        break;
                    }
                    Ok(n) => {
                        let data = bytes::Bytes::copy_from_slice(&tcp_buf[..n]);
                        let msg = Message::Data(DataMessage { conn_id, data });
                        if work_framed.send(msg).await.is_err() {
                            tracing::debug!("连接 {} 工作连接发送失败", conn_id);
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            // 从工作 Framed 连接读取消息，解包后写回用户 TCP
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            // 客户端关闭连接
                            tracing::debug!("连接 {} 客户端关闭", conn_id);
                            let _ = user_tcp.shutdown().await;
                            break;
                        }
                        if user_tcp.write_all(&data_msg.data).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Control(ControlMessage::Pong))) => {
                        // 忽略心跳
                    }
                    Some(Ok(_)) => {
                        // 其他控制消息忽略
                    }
                    Some(Err(e)) => {
                        tracing::debug!("连接 {} 工作连接读取错误: {}", conn_id, e);
                        break;
                    }
                    None => {
                        tracing::debug!("连接 {} 工作连接关闭", conn_id);
                        break;
                    }
                }
            }
        }
    }
}
