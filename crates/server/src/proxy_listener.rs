//! 服务端公网监听器
//!
//! 为每种代理协议创建公网监听器：
//! - TCP: 每个规则独立端口，外部连接 → 工作连接 → 客户端本地
//! - UDP: 每个规则独立端口，外部数据包 → 工作连接 → 客户端本地
//! - HTTP: 共享端口，基于 Host 头路由到对应客户端
//! - HTTPS: 共享端口，基于 SNI 路由 + TLS 终止后转 HTTP
//!
//! 核心原理：所有代理类型统一走"工作连接"模式
//! 服务端收到外部数据 → 通知客户端建立工作连接 → 数据通过工作连接的 DataMessage 双向转发

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_util::codec::Framed;

use rustproxy_core::config::ProxyRule;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{ControlMessage, DataMessage, Message, NewWorkConnRequest};

use crate::client_session::ClientSessionManager;

/// 连接 ID 计数器（全局递增）
static CONN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Framed 代理连接（用于双向数据转发）
type FramedProxyConn = Framed<tokio_rustls::server::TlsStream<tokio::net::TcpStream>, FrameCodec>;

/// 流量统计回调类型
pub type TrafficStatsFn = Arc<dyn Fn(String, u64, u64) + Send + Sync>;

/// 代理监听管理器
#[derive(Clone)]
pub struct ProxyListenerManager {
    /// 独立端口监听器（TCP/UDP）：proxy_name → JoinHandle
    listeners: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
    /// HTTP/HTTPS 共享端口监听器句柄
    http_listener: Arc<RwLock<Option<JoinHandle<()>>>>,
    https_listener: Arc<RwLock<Option<JoinHandle<()>>>>,
    /// 域名路由表：domain → ProxyRule
    domain_routes: Arc<RwLock<HashMap<String, ProxyRule>>>,
    session_manager: ClientSessionManager,
    /// 工作连接等待队列：conn_id → oneshot sender
    pending_work_conns: Arc<RwLock<HashMap<u64, tokio::sync::oneshot::Sender<FramedProxyConn>>>>,
    /// 流量统计回调
    traffic_stats_fn: Arc<RwLock<Option<TrafficStatsFn>>>,
}

impl std::fmt::Debug for ProxyListenerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyListenerManager").finish()
    }
}

impl ProxyListenerManager {
    pub fn new(session_manager: ClientSessionManager) -> Self {
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
            http_listener: Arc::new(RwLock::new(None)),
            https_listener: Arc::new(RwLock::new(None)),
            domain_routes: Arc::new(RwLock::new(HashMap::new())),
            session_manager,
            pending_work_conns: Arc::new(RwLock::new(HashMap::new())),
            traffic_stats_fn: Arc::new(RwLock::new(None)),
        }
    }

    /// 设置流量统计回调
    pub async fn set_traffic_stats_fn(&self, f: TrafficStatsFn) {
        let mut slot = self.traffic_stats_fn.write().await;
        *slot = Some(f);
    }

    /// 报告流量统计
    async fn report_traffic(&self, proxy_name: &str, bytes_in: u64, bytes_out: u64) {
        let cb = self.traffic_stats_fn.read().await.clone();
        if let Some(f) = cb {
            f(proxy_name.to_string(), bytes_in, bytes_out);
        }
    }

    // ============================================================
    // TCP 代理
    // ============================================================

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
                            if let Err(e) = mgr
                                .handle_stream_proxy_conn(&proxy_name, &cid, stream)
                                .await
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

        let mut inner = self.listeners.write().await;
        inner.insert(name_for_insert, handle);
        Ok(())
    }

    // ============================================================
    // UDP 代理
    // ============================================================

    /// 启动 UDP 代理监听器
    ///
    /// UDP 是无连接协议，但内网穿透的本质是数据映射，与 TCP 一致：
    /// 服务端监听 remote_port → 数据通过工作连接隧道转发 → 客户端发到本地端口
    ///
    /// 每个 UDP 外部源地址分配一个 conn_id，对应一个工作连接。
    /// 所有数据（包括后续包）都通过工作连接的 DataMessage 传输，不走主连接。
    pub async fn start_udp_listener(
        &self,
        name: String,
        remote_port: u16,
        bind_addr: String,
        client_id: String,
    ) -> anyhow::Result<()> {
        let addr = format!("{}:{}", bind_addr, remote_port);
        let socket = Arc::new(UdpSocket::bind(&addr).await?);
        tracing::info!("UDP 代理监听已启动: {} -> 端口 {}", name, remote_port);

        let name_for_insert = name.clone();
        let mgr = self.clone();
        let handle = tokio::spawn(async move {
            // UDP 会话表：src_addr → conn_id
            let sessions: Arc<RwLock<HashMap<std::net::SocketAddr, u64>>> =
                Arc::new(RwLock::new(HashMap::new()));
            // UDP 会话通道：conn_id → mpsc sender（用于向工作连接发送数据）
            let session_senders: Arc<
                RwLock<HashMap<u64, tokio::sync::mpsc::Sender<bytes::Bytes>>>,
            > = Arc::new(RwLock::new(HashMap::new()));

            let mut buf = vec![0u8; 65535];

            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, src_addr)) => {
                        let data = bytes::Bytes::copy_from_slice(&buf[..n]);
                        let sessions = sessions.clone();
                        let session_senders = session_senders.clone();
                        let mgr = mgr.clone();
                        let proxy_name = name.clone();
                        let client_id = client_id.clone();
                        let socket = socket.clone();

                        tokio::spawn(async move {
                            mgr.handle_udp_packet(
                                &proxy_name,
                                &client_id,
                                src_addr,
                                data,
                                &sessions,
                                &session_senders,
                                &socket,
                            )
                            .await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!("UDP 代理 {} 接收数据失败: {}", name, e);
                    }
                }
            }
        });

        let mut inner = self.listeners.write().await;
        inner.insert(name_for_insert, handle);
        Ok(())
    }

    /// 处理 UDP 数据包
    ///
    /// 统一走工作连接模式：
    /// - 新源地址：分配 conn_id，发送 NewWorkConn，等客户端建立工作连接后转发
    /// - 已有源地址：查找对应会话的 mpsc channel，直接发送数据
    #[allow(clippy::too_many_arguments)]
    async fn handle_udp_packet(
        &self,
        proxy_name: &str,
        client_id: &str,
        src_addr: std::net::SocketAddr,
        data: bytes::Bytes,
        sessions: &Arc<RwLock<HashMap<std::net::SocketAddr, u64>>>,
        session_senders: &Arc<RwLock<HashMap<u64, tokio::sync::mpsc::Sender<bytes::Bytes>>>>,
        socket: &Arc<UdpSocket>,
    ) {
        // 统计由 udp_session_worker 在会话结束时统一报告

        // 查找已有会话
        let conn_id = {
            let ss = sessions.read().await;
            ss.get(&src_addr).copied()
        };

        if let Some(cid) = conn_id {
            // 已有会话：通过 mpsc channel 发送数据到工作连接任务
            let senders = session_senders.read().await;
            if let Some(tx) = senders.get(&cid) {
                if tx.send(data).await.is_err() {
                    // channel 已关闭，清理会话
                    drop(senders);
                    let mut ss = sessions.write().await;
                    ss.remove(&src_addr);
                    let mut senders = session_senders.write().await;
                    senders.remove(&cid);
                    tracing::debug!("UDP 会话 {} 已过期，清理", cid);
                }
            }
            return;
        }

        // 新会话：分配 conn_id，请求客户端建立工作连接
        let new_conn_id = CONN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

        {
            let mut ss = sessions.write().await;
            ss.insert(src_addr, new_conn_id);
        }

        // 通知客户端建立工作连接
        let work_req = NewWorkConnRequest {
            proxy_name: proxy_name.to_string(),
            conn_id: new_conn_id,
        };
        let sent = self
            .session_manager
            .send_to(
                client_id,
                Message::Control(ControlMessage::NewWorkConn(work_req)),
            )
            .await;

        if !sent {
            let mut ss = sessions.write().await;
            ss.remove(&src_addr);
            return;
        }

        // 等待工作连接建立
        let (tx, rx) = tokio::sync::oneshot::channel::<FramedProxyConn>();
        {
            let mut pending = self.pending_work_conns.write().await;
            pending.insert(new_conn_id, tx);
        }

        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(work_framed)) => {
                // 创建 mpsc channel，用于向工作连接发送数据
                let (data_tx, data_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(256);

                // 注册会话 sender
                {
                    let mut senders = session_senders.write().await;
                    senders.insert(new_conn_id, data_tx.clone());
                }

                // 发送第一个数据包
                if data_tx.send(data).await.is_err() {
                    tracing::warn!("UDP 会话 {} 第一个包发送失败", new_conn_id);
                    return;
                }

                // 启动 UDP 会话管理任务
                let proxy_name_owned = proxy_name.to_string();
                let socket = socket.clone();
                let mgr = self.clone();
                let sessions_clone = sessions.clone();
                let session_senders_clone = session_senders.clone();
                tokio::spawn(async move {
                    udp_session_worker(
                        new_conn_id,
                        src_addr,
                        work_framed,
                        data_rx,
                        &socket,
                        &proxy_name_owned,
                        &mgr,
                    )
                    .await;

                    // 会话结束，清理
                    {
                        let mut ss = sessions_clone.write().await;
                        ss.remove(&src_addr);
                    }
                    {
                        let mut senders = session_senders_clone.write().await;
                        senders.remove(&new_conn_id);
                    }
                });
            }
            _ => {
                // 超时或错误，清理
                let mut pending = self.pending_work_conns.write().await;
                pending.remove(&new_conn_id);
                let mut ss = sessions.write().await;
                ss.remove(&src_addr);
                tracing::debug!("UDP 会话 {} 工作连接建立超时", new_conn_id);
            }
        }
    }

    // ============================================================
    // HTTP 代理（基于域名路由）
    // ============================================================

    /// 注册 HTTP/HTTPS 域名路由
    pub async fn register_domain_route(&self, rule: &ProxyRule) {
        let mut routes = self.domain_routes.write().await;
        for domain in &rule.custom_domains {
            routes.insert(domain.clone(), rule.clone());
            tracing::info!("域名路由已注册: {} -> {}", domain, rule.name);
        }
    }

    /// 移除 HTTP/HTTPS 域名路由
    pub async fn unregister_domain_route(&self, rule: &ProxyRule) {
        let mut routes = self.domain_routes.write().await;
        for domain in &rule.custom_domains {
            routes.remove(domain);
            tracing::info!("域名路由已移除: {} -> {}", domain, rule.name);
        }
    }

    /// 启动 HTTP 共享监听器
    pub async fn start_http_listener(&self, bind_addr: String, port: u16) -> anyhow::Result<()> {
        let addr = format!("{}:{}", bind_addr, port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("HTTP 代理监听已启动: {}", addr);

        let mgr = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        tracing::debug!("HTTP 代理收到连接: {}", peer_addr);
                        let mgr = mgr.clone();
                        tokio::spawn(async move {
                            if let Err(e) = mgr.handle_http_proxy_conn(stream).await {
                                tracing::debug!("HTTP 代理连接处理失败: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("HTTP 代理接受连接失败: {}", e);
                    }
                }
            }
        });

        let mut hl = self.http_listener.write().await;
        *hl = Some(handle);
        Ok(())
    }

    /// 处理 HTTP 代理连接
    async fn handle_http_proxy_conn(&self, stream: TcpStream) -> anyhow::Result<()> {
        // 1. 先读取足够的数据来解析 Host 头
        let mut peek_buf = vec![0u8; 4096];
        let n = stream.peek(&mut peek_buf).await?;
        let peek_data = &peek_buf[..n];

        // 2. 解析 Host 头
        let host = extract_host_from_http(peek_data)
            .ok_or_else(|| anyhow::anyhow!("无法解析 HTTP Host 头"))?;

        tracing::debug!("HTTP 代理请求 Host: {}", host);

        // 3. 查找域名路由
        let rule = {
            let routes = self.domain_routes.read().await;
            routes.get(&host).cloned()
        };

        let rule = rule.ok_or_else(|| anyhow::anyhow!("未找到域名 {} 的代理规则", host))?;

        // 4. 像普通 TCP 一样处理
        self.handle_stream_proxy_conn(&rule.name, &rule.client_id, stream)
            .await
    }

    // ============================================================
    // HTTPS 代理（TLS 终止 + SNI 路由）
    // ============================================================

    /// 启动 HTTPS 共享监听器
    pub async fn start_https_listener(
        &self,
        bind_addr: String,
        port: u16,
        tls_config: Arc<rustls::ServerConfig>,
    ) -> anyhow::Result<()> {
        let addr = format!("{}:{}", bind_addr, port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("HTTPS 代理监听已启动: {}", addr);

        let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_config);

        let mgr = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        tracing::debug!("HTTPS 代理收到连接: {}", peer_addr);
                        let mgr = mgr.clone();
                        let acceptor = tls_acceptor.clone();
                        tokio::spawn(async move {
                            // 先 peek SNI
                            let mut peek_buf = vec![0u8; 4096];
                            let n = match stream.peek(&mut peek_buf).await {
                                Ok(n) => n,
                                Err(e) => {
                                    tracing::debug!("HTTPS peek 失败: {}", e);
                                    return;
                                }
                            };

                            let sni = extract_sni_from_client_hello(&peek_buf[..n]);
                            tracing::debug!("HTTPS 代理请求 SNI: {:?}", sni);

                            let rule = match sni {
                                Some(ref domain) => {
                                    let routes = mgr.domain_routes.read().await;
                                    routes.get(domain).cloned()
                                }
                                None => None,
                            };

                            let rule = match rule {
                                Some(r) => r,
                                None => {
                                    tracing::debug!("HTTPS 代理未找到 SNI 匹配的代理规则");
                                    return;
                                }
                            };

                            // TLS 握手
                            let tls_stream = match acceptor.accept(stream).await {
                                Ok(s) => s,
                                Err(e) => {
                                    tracing::debug!("HTTPS TLS 握手失败: {}", e);
                                    return;
                                }
                            };

                            // TLS 终止后，像普通 TCP 一样桥接
                            if let Err(e) = mgr
                                .handle_stream_proxy_conn(&rule.name, &rule.client_id, tls_stream)
                                .await
                            {
                                tracing::debug!("HTTPS 代理连接处理失败: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("HTTPS 代理接受连接失败: {}", e);
                    }
                }
            }
        });

        let mut hl = self.https_listener.write().await;
        *hl = Some(handle);
        Ok(())
    }

    // ============================================================
    // 通用连接处理（TCP / HTTP / HTTPS TLS 终止后）
    // ============================================================

    /// 处理基于流的代理连接
    ///
    /// 统一流程：分配 conn_id → 通知客户端建立工作连接 → 等待 → 双向转发
    async fn handle_stream_proxy_conn<S>(
        &self,
        proxy_name: &str,
        client_id: &str,
        user_stream: S,
    ) -> anyhow::Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
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
            let mut pending = self.pending_work_conns.write().await;
            pending.remove(&conn_id);
            anyhow::bail!("客户端 {} 不在线，无法建立工作连接", client_id);
        }

        // 5. 等待客户端建立工作连接（30 秒超时）
        let work_framed = match tokio::time::timeout(Duration::from_secs(30), rx).await {
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

        // 6. 双向数据转发
        let proxy_name_owned = proxy_name.to_string();
        let mgr = self.clone();
        tokio::spawn(async move {
            let (bytes_in, bytes_out) =
                bridge_stream_to_framed(user_stream, work_framed, conn_id, &mgr, &proxy_name_owned).await;
            tracing::info!(
                "代理 {} 连接 {} 已结束 (↑{}B ↓{}B)",
                proxy_name_owned,
                conn_id,
                bytes_in,
                bytes_out
            );
        });

        Ok(())
    }

    // ============================================================
    // 管理
    // ============================================================

    /// 注册工作连接（客户端发起的工作 TLS 连接）
    pub async fn register_work_conn(&self, conn_id: u64, framed: FramedProxyConn) -> bool {
        let mut pending = self.pending_work_conns.write().await;
        if let Some(sender) = pending.remove(&conn_id) {
            if sender.send(framed).is_ok() {
                tracing::debug!("工作连接 {} 已匹配到等待的代理", conn_id);
                return true;
            }
        }
        tracing::warn!("工作连接 {} 没有匹配的等待代理", conn_id);
        false
    }

    /// 停止独立端口监听器
    pub async fn stop_listener(&self, name: &str) {
        let mut inner = self.listeners.write().await;
        if let Some(handle) = inner.remove(name) {
            handle.abort();
            tracing::info!("代理监听 {} 已停止", name);
        }
    }

    /// 停止所有监听器
    pub async fn stop_all(&self) {
        let mut inner = self.listeners.write().await;
        for (name, handle) in inner.drain() {
            handle.abort();
            tracing::debug!("代理监听 {} 已停止", name);
        }

        let mut hl = self.http_listener.write().await;
        if let Some(h) = hl.take() {
            h.abort();
        }

        let mut hl = self.https_listener.write().await;
        if let Some(h) = hl.take() {
            h.abort();
        }
    }
}

// ============================================================
// UDP 会话工作器
// ============================================================

/// UDP 会话工作器
///
/// 管理一个 UDP "会话"（一个外部源地址对应一个工作连接）：
/// - 从 mpsc channel 接收外部用户数据 → 通过工作连接发送到客户端
/// - 从工作连接读取客户端返回的数据 → 通过 UdpSocket 发回外部用户
async fn udp_session_worker(
    conn_id: u64,
    peer_addr: std::net::SocketAddr,
    mut work_framed: FramedProxyConn,
    mut data_rx: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    socket: &Arc<UdpSocket>,
    proxy_name: &str,
    mgr: &ProxyListenerManager,
) {
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;

    loop {
        tokio::select! {
            // 从 mpsc channel 接收外部用户数据，通过工作连接发送到客户端
            data = data_rx.recv() => {
                match data {
                    Some(data) => {
                        let n = data.len() as u64;
                        bytes_in += n;
                        if work_framed.send(Message::Data(DataMessage {
                            conn_id,
                            data,
                        })).await.is_err() {
                            tracing::debug!("UDP 会话 {} 工作连接发送失败", conn_id);
                            break;
                        }
                        // 实时报告入站流量
                        mgr.report_traffic(proxy_name, n, 0).await;
                    }
                    None => {
                        // channel 关闭，会话结束
                        break;
                    }
                }
            }
            // 从工作连接读取客户端返回的数据，通过 UdpSocket 发回外部用户
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            tracing::debug!("UDP 会话 {} 客户端关闭", conn_id);
                            break;
                        }
                        let n = data_msg.data.len() as u64;
                        bytes_out += n;
                        if socket.send_to(&data_msg.data, peer_addr).await.is_err() {
                            tracing::debug!("UDP 会话 {} 发送回外部用户失败", conn_id);
                            break;
                        }
                        // 实时报告出站流量
                        mgr.report_traffic(proxy_name, 0, n).await;
                    }
                    Some(Ok(Message::Control(ControlMessage::Pong))) => {
                        // 心跳，忽略
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("UDP 会话 {} 工作连接读取错误: {}", conn_id, e);
                        break;
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    tracing::debug!("UDP 会话 {} 结束 (↑{}B ↓{}B)", conn_id, bytes_in, bytes_out);
}

// ============================================================
// TCP 双向转发
// ============================================================

/// 双向转发：AsyncRead+AsyncWrite 流 ↔ Framed TLS 工作连接
///
/// 逐 chunk 上报流量（用于实时带宽计算），返回 (bytes_in, bytes_out) 用于日志。
/// bytes_in = 从用户流读取的字节数（用户 → 客户端）
/// bytes_out = 写入用户流的字节数（客户端 → 用户）
async fn bridge_stream_to_framed<S>(
    mut user_stream: S,
    mut work_framed: FramedProxyConn,
    conn_id: u64,
    mgr: &ProxyListenerManager,
    proxy_name: &str,
) -> (u64, u64)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let mut buf = vec![0u8; 8192];
    let mut bytes_in: u64 = 0;
    let mut bytes_out: u64 = 0;

    loop {
        tokio::select! {
            // 从用户流读取数据，通过 Framed 发送到客户端
            result = user_stream.read(&mut buf) => {
                match result {
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
                        bytes_in += n as u64;
                        let data = bytes::Bytes::copy_from_slice(&buf[..n]);
                        if work_framed.send(Message::Data(DataMessage { conn_id, data })).await.is_err() {
                            break;
                        }
                        // 逐 chunk 上报入站流量（用于实时带宽计算）
                        mgr.report_traffic(proxy_name, n as u64, 0).await;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            // 从工作 Framed 连接读取消息，解包后写回用户流
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            // 客户端关闭连接
                            tracing::debug!("连接 {} 客户端关闭", conn_id);
                            let _ = user_stream.shutdown().await;
                            break;
                        }
                        let n = data_msg.data.len() as u64;
                        bytes_out += n;
                        if user_stream.write_all(&data_msg.data).await.is_err() {
                            break;
                        }
                        // 逐 chunk 上报出站流量（用于实时带宽计算）
                        mgr.report_traffic(proxy_name, 0, n).await;
                    }
                    Some(Ok(Message::Control(ControlMessage::Pong))) => {
                        // 忽略心跳
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("连接 {} 工作连接读取错误: {}", conn_id, e);
                        break;
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    (bytes_in, bytes_out)
}

// ============================================================
// HTTP Host 头解析
// ============================================================

/// 从 HTTP 请求数据中提取 Host 头
fn extract_host_from_http(data: &[u8]) -> Option<String> {
    let header = std::str::from_utf8(data).ok()?;

    for line in header.lines() {
        if let Some(host_value) = line.strip_prefix("Host:") {
            let host = host_value.trim();
            // 去掉端口部分
            let host = if host.starts_with('[') {
                // IPv6: [::1]:port
                host.find(']').map(|i| &host[..=i]).unwrap_or(host)
            } else {
                host.split(':').next().unwrap_or(host)
            };
            return Some(host.to_string());
        }
        // HTTP 请求头以空行结束
        if line.is_empty() {
            break;
        }
    }
    None
}

// ============================================================
// TLS ClientHello SNI 解析
// ============================================================

/// 从 TLS ClientHello 中提取 SNI（Server Name Indication）
fn extract_sni_from_client_hello(data: &[u8]) -> Option<String> {
    // TLS Record Header: 5 bytes
    // Content Type (1) + Version (2) + Length (2)
    if data.len() < 44 {
        return None;
    }

    // 检查是否是 ClientHello (ContentType = 0x16, HandshakeType = 0x01)
    if data[0] != 0x16 {
        return None;
    }

    // Handshake type offset = 5
    if data[5] != 0x01 {
        return None;
    }

    // Session ID 长度偏移 = 43 (5 + 1 + 3 + 2 + 32 = 43)
    let session_id_len = *data.get(43)? as usize;
    let cipher_suites_offset = 44 + session_id_len;
    if cipher_suites_offset + 2 > data.len() {
        return None;
    }

    let cipher_suites_len =
        u16::from_be_bytes([data[cipher_suites_offset], data[cipher_suites_offset + 1]]) as usize;
    let compression_offset = cipher_suites_offset + 2 + cipher_suites_len;
    if compression_offset + 2 > data.len() {
        return None;
    }

    let compression_len = data[compression_offset] as usize;
    let extensions_offset = compression_offset + 1 + compression_len;
    if extensions_offset + 2 > data.len() {
        return None;
    }

    let extensions_total_len =
        u16::from_be_bytes([data[extensions_offset], data[extensions_offset + 1]]) as usize;
    let mut offset = extensions_offset + 2;
    let extensions_end = offset + extensions_total_len;

    // 遍历扩展查找 SNI (extension type = 0x0000)
    while offset + 4 <= extensions_end.min(data.len()) {
        let ext_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;

        if ext_type == 0x0000 {
            // SNI extension
            let sni_list_offset = offset + 4;
            if sni_list_offset + 3 > data.len() {
                return None;
            }
            let _sni_list_len =
                u16::from_be_bytes([data[sni_list_offset], data[sni_list_offset + 1]]);
            let name_type = data[sni_list_offset + 2];
            if name_type != 0x00 {
                return None;
            }
            let name_len_offset = sni_list_offset + 3;
            if name_len_offset + 2 > data.len() {
                return None;
            }
            let name_len =
                u16::from_be_bytes([data[name_len_offset], data[name_len_offset + 1]]) as usize;
            let name_start = name_len_offset + 2;
            if name_start + name_len > data.len() {
                return None;
            }
            let name = std::str::from_utf8(&data[name_start..name_start + name_len]).ok()?;
            return Some(name.to_string());
        }

        offset += 4 + ext_len;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host_from_http() {
        let req = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert_eq!(extract_host_from_http(req), Some("example.com".to_string()));

        let req = b"GET / HTTP/1.1\r\nHost: example.com:8080\r\n\r\n";
        assert_eq!(extract_host_from_http(req), Some("example.com".to_string()));

        let req = b"GET / HTTP/1.1\r\n\r\n";
        assert_eq!(extract_host_from_http(req), None);

        let req = b"garbage";
        assert_eq!(extract_host_from_http(req), None);
    }

    #[test]
    fn test_extract_sni_from_client_hello_too_short() {
        let data = [0u8; 10];
        assert_eq!(extract_sni_from_client_hello(&data), None);
    }

    #[test]
    fn test_extract_sni_not_client_hello() {
        let mut data = vec![0u8; 100];
        data[0] = 0x15; // Not Handshake
        assert_eq!(extract_sni_from_client_hello(&data), None);
    }
}
