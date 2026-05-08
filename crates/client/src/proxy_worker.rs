//! 代理工作管理
//!
//! 管理客户端本地的代理工作进程：接收服务端指令，建立本地连接，转发数据。
//!
//! 数据流：
//! 1. 服务端收到外部用户连接 → 通知客户端建立工作连接（NewWorkConn）
//! 2. 客户端新建 TLS 连接到服务端，发送 NewWorkConnResp 确认
//! 3. 客户端连接本地服务
//! 4. 双向转发：本地服务 ↔ 工作连接
//!
//! TCP/HTTP/HTTPS: 连接本地 TCP 服务
//! UDP: 连接本地 UDP 服务

use std::collections::HashMap;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

use rustproxy_core::config::ClientConfig;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{ControlMessage, DataMessage, Message, ServerAssignProxyRequest};

/// 代理工作管理器
#[derive(Debug, Clone)]
pub struct ProxyWorkerManager {
    /// 代理名称 -> 本地地址映射
    local_addrs: Arc<RwLock<HashMap<String, String>>>,
    /// 代理名称 -> 代理类型
    proxy_types: Arc<RwLock<HashMap<String, String>>>,
    /// 代理名称 -> PROXY Protocol 版本
    proxy_protocols: Arc<RwLock<HashMap<String, String>>>,
    /// 代理名称 -> 工作任务句柄
    tasks: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
}

impl ProxyWorkerManager {
    pub fn new() -> Self {
        Self {
            local_addrs: Arc::new(RwLock::new(HashMap::new())),
            proxy_types: Arc::new(RwLock::new(HashMap::new())),
            proxy_protocols: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动代理工作进程（注册本地地址和类型）
    pub async fn start_proxy(
        &self,
        req: ServerAssignProxyRequest,
        _config: &ClientConfig,
    ) -> anyhow::Result<()> {
        let local_addr = format!("{}:{}", req.local_ip, req.local_port);

        // 注册本地地址映射和类型
        {
            let mut addrs = self.local_addrs.write().await;
            addrs.insert(req.name.clone(), local_addr.clone());
        }
        {
            let mut types = self.proxy_types.write().await;
            types.insert(req.name.clone(), req.proxy_type.clone());
        }
        {
            let mut protocols = self.proxy_protocols.write().await;
            protocols.insert(req.name.clone(), req.proxy_protocol.clone());
        }

        tracing::info!(
            "代理规则已注册: {} -> {} (type={}, proxy_protocol={})",
            req.name,
            local_addr,
            req.proxy_type,
            req.proxy_protocol
        );
        Ok(())
    }

    /// 停止代理工作进程
    pub async fn stop_proxy(&self, name: &str) {
        let mut addrs = self.local_addrs.write().await;
        addrs.remove(name);
        let mut types = self.proxy_types.write().await;
        types.remove(name);
        let mut protocols = self.proxy_protocols.write().await;
        protocols.remove(name);

        let mut tasks = self.tasks.write().await;
        if let Some(handle) = tasks.remove(name) {
            handle.abort();
        }
        tracing::info!("代理 {} 已停止", name);
    }

    /// 停止所有代理工作进程
    pub async fn stop_all(&self) {
        let mut addrs = self.local_addrs.write().await;
        addrs.clear();
        let mut types = self.proxy_types.write().await;
        types.clear();
        let mut protocols = self.proxy_protocols.write().await;
        protocols.clear();

        let mut tasks = self.tasks.write().await;
        for (name, handle) in tasks.drain() {
            handle.abort();
            tracing::debug!("代理 {} 已停止", name);
        }
    }

    /// 获取代理规则的本地地址
    pub async fn get_local_addr(&self, name: &str) -> Option<String> {
        let addrs = self.local_addrs.read().await;
        addrs.get(name).cloned()
    }

    /// 获取代理规则的类型
    pub async fn get_proxy_type(&self, name: &str) -> Option<String> {
        let types = self.proxy_types.read().await;
        types.get(name).cloned()
    }

    /// 获取代理规则的 PROXY Protocol 版本
    pub async fn get_proxy_protocol(&self, name: &str) -> Option<String> {
        let protocols = self.proxy_protocols.read().await;
        protocols.get(name).cloned()
    }
}

impl Default for ProxyWorkerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 打开工作连接（收到 NewWorkConn 后调用）
///
/// 根据代理类型选择不同的本地连接方式：
/// - TCP/HTTP/HTTPS: 连接本地 TCP 服务，双向转发
/// - UDP: 连接本地 UDP 服务，双向转发
pub async fn open_work_connection(
    config: &ClientConfig,
    proxy_name: &str,
    conn_id: u64,
    local_addr: &str,
    proxy_type: &str,
    user_addr: Option<String>,
    proxy_protocol: Option<String>,
) -> anyhow::Result<()> {
    tracing::info!(
        "建立工作连接: proxy={}, conn_id={}, local={}, type={}, user_addr={:?}, proxy_protocol={:?}",
        proxy_name,
        conn_id,
        local_addr,
        proxy_type,
        user_addr,
        proxy_protocol
    );

    // 1. 建立新的 TLS 连接到服务端
    let addr = format!(
        "{}:{}",
        config.client.server_addr, config.client.server_port
    );
    let tcp_stream = tokio::net::TcpStream::connect(&addr).await?;

    let tls_config = build_work_tls_config(&config.client.ca_cert)?;
    let connector = tokio_rustls::TlsConnector::from(tls_config);
    let domain = crate::connector::resolve_server_name(
        &config.client.server_name,
        &config.client.server_addr,
    )?;
    let tls_stream = connector.connect(domain, tcp_stream).await?;

    // 2. 创建 Framed 并发送确认
    let mut framed = Framed::new(tls_stream, FrameCodec);

    let resp = rustproxy_proto::message::NewWorkConnResponse {
        proxy_name: proxy_name.to_string(),
        conn_id,
        success: true,
    };
    framed
        .send(Message::Control(
            rustproxy_proto::message::ControlMessage::NewWorkConnResp(resp),
        ))
        .await?;

    // 3. 根据代理类型连接本地服务
    match proxy_type {
        "udp" => {
            bridge_udp_to_framed(local_addr, framed, conn_id).await;
        }
        _ => {
            // TCP / HTTP / HTTPS 都走 TCP
            let mut local_stream = match tokio::net::TcpStream::connect(local_addr).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("连接本地服务失败 {} -> {}: {}", proxy_name, local_addr, e);
                    let err_msg = Message::Data(DataMessage {
                        conn_id,
                        data: bytes::Bytes::new(),
                    });
                    let _ = framed.send(err_msg).await;
                    return Err(e.into());
                }
            };

            // 注入 PROXY Protocol 头（在数据转发前写入）
            let pp_version = proxy_protocol.as_deref().unwrap_or("");
            if !pp_version.is_empty() {
                if let Err(e) =
                    inject_proxy_protocol(&mut local_stream, &user_addr, pp_version).await
                {
                    tracing::warn!(
                        "代理 {} PROXY Protocol 注入失败: {}，继续不注入",
                        proxy_name,
                        e
                    );
                }
            }

            bridge_local_tcp_to_framed(&mut local_stream, &mut framed, conn_id).await;
        }
    }

    tracing::info!("工作连接已结束: proxy={}, conn_id={}", proxy_name, conn_id);
    Ok(())
}

/// 注入 PROXY Protocol 头到本地连接
///
/// 在 TCP 连接建立后、业务数据传输前，写入 PROXY Protocol 头，
/// 让后端服务能获取到用户的真实 IP 地址。
async fn inject_proxy_protocol(
    local_stream: &mut tokio::net::TcpStream,
    user_addr: &Option<String>,
    version: &str,
) -> anyhow::Result<()> {
    let header = match version {
        "v1" => build_proxy_protocol_v1(user_addr)?,
        "v2" => build_proxy_protocol_v2(user_addr)?,
        other => anyhow::bail!("不支持的 PROXY Protocol 版本: {}", other),
    };
    local_stream.write_all(&header).await?;
    tracing::debug!(
        "PROXY Protocol {} 头已注入 ({} bytes)",
        version,
        header.len()
    );
    Ok(())
}

/// 构建 PROXY Protocol v1 头（文本格式）
///
/// 格式: `PROXY TCP4 src_ip dst_ip src_port dst_port\r\n`
/// 或:   `PROXY TCP6 src_ip dst_ip src_port dst_port\r\n`
/// 或:   `PROXY UNKNOWN\r\n`（无法确定地址时）
fn build_proxy_protocol_v1(user_addr: &Option<String>) -> anyhow::Result<Vec<u8>> {
    match user_addr {
        Some(addr_str) => {
            // 解析 "ip:port" 格式
            let addr: std::net::SocketAddr = addr_str
                .parse()
                .map_err(|e| anyhow::anyhow!("解析用户地址失败 {}: {}", addr_str, e))?;

            match addr {
                std::net::SocketAddr::V4(v4) => {
                    let header = format!(
                        "PROXY TCP4 {} 127.0.0.1 {} {}\r\n",
                        v4.ip(),
                        v4.port(),
                        0 // 本地端口未知，填 0
                    );
                    Ok(header.into_bytes())
                }
                std::net::SocketAddr::V6(v6) => {
                    let header = format!("PROXY TCP6 {} ::1 {} {}\r\n", v6.ip(), v6.port(), 0);
                    Ok(header.into_bytes())
                }
            }
        }
        None => {
            // 无法获取用户地址，发送 UNKNOWN
            Ok(b"PROXY UNKNOWN\r\n".to_vec())
        }
    }
}

/// 构建 PROXY Protocol v2 头（二进制格式）
///
/// 格式:
/// - 12 字节签名: \x0d\x0a\x0d\x0a\x00\x0d\x0a\x51\x55\x49\x54\x0a
/// - 1 字节: 版本(4bit) + 命令(4bit)，0x21 = PROXY
/// - 1 字节: 地址族(4bit) + 协议(4bit)，0x11 = IPv4+STREAM, 0x21 = IPv6+STREAM
/// - 2 字节: 地址长度（大端）
/// - 地址数据: src_ip + dst_ip + src_port + dst_port
fn build_proxy_protocol_v2(user_addr: &Option<String>) -> anyhow::Result<Vec<u8>> {
    const SIGNATURE: [u8; 12] = [
        0x0d, 0x0a, 0x0d, 0x0a, 0x00, 0x0d, 0x0a, 0x51, 0x55, 0x49, 0x54, 0x0a,
    ];
    const CMD_PROXY: u8 = 0x21; // version 2 | PROXY command

    match user_addr {
        Some(addr_str) => {
            let addr: std::net::SocketAddr = addr_str
                .parse()
                .map_err(|e| anyhow::anyhow!("解析用户地址失败 {}: {}", addr_str, e))?;

            match addr {
                std::net::SocketAddr::V4(v4) => {
                    let af_proto: u8 = 0x11; // AF_INET | SOCK_STREAM
                    let addr_len: u16 = 12; // 2*4 + 2*2
                    let mut buf = Vec::with_capacity(12 + 4 + 12);
                    buf.extend_from_slice(&SIGNATURE);
                    buf.push(CMD_PROXY);
                    buf.push(af_proto);
                    buf.extend_from_slice(&addr_len.to_be_bytes());
                    buf.extend_from_slice(&v4.ip().octets());
                    buf.extend_from_slice(&[127, 0, 0, 1]); // dst IP: 127.0.0.1
                    buf.extend_from_slice(&v4.port().to_be_bytes());
                    buf.extend_from_slice(&0u16.to_be_bytes()); // dst port
                    Ok(buf)
                }
                std::net::SocketAddr::V6(v6) => {
                    let af_proto: u8 = 0x21; // AF_INET6 | SOCK_STREAM
                    let addr_len: u16 = 36; // 2*16 + 2*2
                    let mut buf = Vec::with_capacity(12 + 4 + 36);
                    buf.extend_from_slice(&SIGNATURE);
                    buf.push(CMD_PROXY);
                    buf.push(af_proto);
                    buf.extend_from_slice(&addr_len.to_be_bytes());
                    buf.extend_from_slice(&v6.ip().octets());
                    buf.extend_from_slice(&[0u8; 16]); // dst IP: ::1
                    buf.extend_from_slice(&v6.port().to_be_bytes());
                    buf.extend_from_slice(&0u16.to_be_bytes()); // dst port
                    Ok(buf)
                }
            }
        }
        None => {
            // UNKNOWN: cmd = 0x20, af_proto = 0x00, addr_len = 0
            let mut buf = Vec::with_capacity(16);
            buf.extend_from_slice(&SIGNATURE);
            buf.push(0x20); // version 2 | LOCAL command
            buf.push(0x00); // AF_UNSPEC | UNSPEC
            buf.extend_from_slice(&0u16.to_be_bytes());
            Ok(buf)
        }
    }
}

/// 双向转发：本地 TCP 连接 ↔ Framed TLS 工作连接
async fn bridge_local_tcp_to_framed(
    local_tcp: &mut tokio::net::TcpStream,
    work_framed: &mut Framed<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, FrameCodec>,
    conn_id: u64,
) {
    use tokio::io::AsyncReadExt;

    let mut buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            // 从本地 TCP 读取数据，通过 Framed 发送到服务端
            result = local_tcp.read(&mut buf) => {
                match result {
                    Ok(0) => {
                        tracing::debug!("连接 {} 本地服务关闭", conn_id);
                        let _ = work_framed.send(Message::Data(DataMessage {
                            conn_id,
                            data: bytes::Bytes::new(),
                        })).await;
                        let _ = work_framed.close().await;
                        break;
                    }
                    Ok(n) => {
                        let data = bytes::Bytes::copy_from_slice(&buf[..n]);
                        if work_framed.send(Message::Data(DataMessage { conn_id, data })).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            // 从工作 Framed 连接读取消息，解包后写回本地 TCP
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            tracing::debug!("连接 {} 服务端关闭", conn_id);
                            let _ = local_tcp.shutdown().await;
                            break;
                        }
                        if local_tcp.write_all(&data_msg.data).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Control(ControlMessage::Pong))) => {}
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
}

/// 双向转发：本地 UDP 服务 ↔ Framed TLS 工作连接
///
/// UDP 代理模式：
/// - 从工作连接收到的 DataMessage → 发送到本地 UDP 服务
/// - 从本地 UDP 服务收到回复 → 通过工作连接发送 DataMessage
async fn bridge_udp_to_framed(
    local_addr: &str,
    mut work_framed: Framed<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, FrameCodec>,
    conn_id: u64,
) {
    // 绑定本地任意端口用于 UDP 通信
    let local_udp = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("绑定本地 UDP 端口失败: {}", e);
            return;
        }
    };

    // 连接到本地 UDP 服务
    if let Err(e) = local_udp.connect(local_addr).await {
        tracing::error!("连接本地 UDP 服务失败 {}: {}", local_addr, e);
        return;
    }

    let mut udp_buf = vec![0u8; 65535];

    loop {
        tokio::select! {
            // 从工作连接读取数据，转发到本地 UDP 服务
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            tracing::debug!("UDP 连接 {} 服务端关闭", conn_id);
                            break;
                        }
                        if let Err(e) = local_udp.send(&data_msg.data).await {
                            tracing::debug!("UDP 连接 {} 发送到本地服务失败: {}", conn_id, e);
                            break;
                        }
                    }
                    Some(Ok(Message::Control(ControlMessage::Pong))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::debug!("UDP 连接 {} 工作连接读取错误: {}", conn_id, e);
                        break;
                    }
                    None => {
                        break;
                    }
                }
            }
            // 从本地 UDP 服务读取回复，通过工作连接发送
            result = local_udp.recv(&mut udp_buf) => {
                match result {
                    Ok(n) => {
                        let data = bytes::Bytes::copy_from_slice(&udp_buf[..n]);
                        if work_framed.send(Message::Data(DataMessage { conn_id, data })).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("UDP 连接 {} 本地服务接收失败: {}", conn_id, e);
                        break;
                    }
                }
            }
        }
    }
}

fn build_work_tls_config(ca_cert_path: &str) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    if ca_cert_path.is_empty() {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(crate::connector::NoVerifier))
            .with_no_client_auth();
        Ok(Arc::new(config))
    } else {
        let mut root_store = rustls::RootCertStore::empty();
        let ca_pem = std::fs::read(ca_cert_path)?;
        let ca_certs =
            rustls_pemfile::certs(&mut &ca_pem[..]).collect::<std::result::Result<Vec<_>, _>>()?;
        for cert in ca_certs {
            root_store.add(cert)?;
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Ok(Arc::new(config))
    }
}
