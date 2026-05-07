//! 代理工作管理
//!
//! 管理客户端本地的代理工作进程：接收服务端指令，建立本地连接，转发数据。
//!
//! 数据流：
//! 1. 服务端收到外部用户连接 → 通知客户端建立工作连接（NewWorkConn）
//! 2. 客户端新建 TLS 连接到服务端，发送 NewWorkConnResp 确认
//! 3. 客户端连接本地服务
//! 4. 双向转发：本地服务 ↔ 工作连接

use std::collections::HashMap;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

use rustproxy_core::config::ClientConfig;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{DataMessage, Message, ServerAssignProxyRequest};

/// 代理工作管理器
#[derive(Debug, Clone)]
pub struct ProxyWorkerManager {
    /// 代理名称 -> 本地地址映射
    local_addrs: Arc<RwLock<HashMap<String, String>>>,
    /// 代理名称 -> 工作任务句柄
    tasks: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
}

impl ProxyWorkerManager {
    pub fn new() -> Self {
        Self {
            local_addrs: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动代理工作进程（注册本地地址）
    pub async fn start_proxy(
        &self,
        req: ServerAssignProxyRequest,
        _config: &ClientConfig,
    ) -> anyhow::Result<()> {
        let local_addr = format!("{}:{}", req.local_ip, req.local_port);

        // 注册本地地址映射
        {
            let mut addrs = self.local_addrs.write().await;
            addrs.insert(req.name.clone(), local_addr.clone());
        }

        tracing::info!(
            "代理规则已注册: {} -> {} (type={})",
            req.name,
            local_addr,
            req.proxy_type
        );
        Ok(())
    }

    /// 停止代理工作进程
    pub async fn stop_proxy(&self, name: &str) {
        let mut addrs = self.local_addrs.write().await;
        addrs.remove(name);

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
}

impl Default for ProxyWorkerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 打开工作连接（收到 NewWorkConn 后调用）
///
/// 工作连接流程：
/// 1. 客户端新建 TLS 连接到服务端
/// 2. 发送 NewWorkConnResp 确认
/// 3. 连接到本地服务
/// 4. 双向转发数据
pub async fn open_work_connection(
    config: &ClientConfig,
    proxy_name: &str,
    conn_id: u64,
    local_addr: &str,
) -> anyhow::Result<()> {
    tracing::info!(
        "建立工作连接: proxy={}, conn_id={}, local={}",
        proxy_name,
        conn_id,
        local_addr
    );

    // 1. 建立新的 TLS 连接到服务端
    let addr = format!(
        "{}:{}",
        config.client.server_addr, config.client.server_port
    );
    let tcp_stream = TcpStream::connect(&addr).await?;

    let tls_config = build_work_tls_config(&config.client.ca_cert)?;
    let connector = tokio_rustls::TlsConnector::from(tls_config);
    let domain = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| anyhow::anyhow!("域名解析失败: {}", e))?;
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

    // 3. 连接到本地服务
    let mut local_stream = match TcpStream::connect(local_addr).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("连接本地服务失败 {} -> {}: {}", proxy_name, local_addr, e);
            // 通知服务端连接失败（发送空 DataMessage）
            let err_msg = Message::Data(DataMessage {
                conn_id,
                data: bytes::Bytes::new(),
            });
            let _ = framed.send(err_msg).await;
            return Err(e.into());
        }
    };

    tracing::info!(
        "工作连接已建立: proxy={}, conn_id={}, local={}",
        proxy_name,
        conn_id,
        local_addr
    );

    // 4. 双向数据转发：本地 TCP ↔ 工作 TLS (Framed)
    bridge_local_to_framed(&mut local_stream, &mut framed, conn_id).await;

    tracing::info!("工作连接已结束: proxy={}, conn_id={}", proxy_name, conn_id);
    Ok(())
}

/// 双向转发：本地 TCP 连接 ↔ Framed TLS 工作连接
async fn bridge_local_to_framed(
    local_tcp: &mut TcpStream,
    work_framed: &mut Framed<tokio_rustls::client::TlsStream<TcpStream>, FrameCodec>,
    conn_id: u64,
) {
    let mut tcp_buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            // 从本地 TCP 读取数据，通过 Framed 发送到服务端
            result = local_tcp.readable() => {
                if result.is_err() {
                    break;
                }
                match local_tcp.try_read(&mut tcp_buf) {
                    Ok(0) => {
                        // 本地服务关闭连接
                        tracing::debug!("连接 {} 本地服务关闭", conn_id);
                        let _ = work_framed.send(Message::Data(DataMessage {
                            conn_id,
                            data: bytes::Bytes::new(),
                        })).await;
                        let _ = work_framed.close().await;
                        break;
                    }
                    Ok(n) => {
                        let data = bytes::Bytes::copy_from_slice(&tcp_buf[..n]);
                        if work_framed.send(Message::Data(DataMessage { conn_id, data })).await.is_err() {
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
            // 从工作 Framed 连接读取消息，解包后写回本地 TCP
            result = work_framed.next() => {
                match result {
                    Some(Ok(Message::Data(data_msg))) => {
                        if data_msg.data.is_empty() {
                            // 服务端关闭连接
                            tracing::debug!("连接 {} 服务端关闭", conn_id);
                            let _ = local_tcp.shutdown().await;
                            break;
                        }
                        if local_tcp.write_all(&data_msg.data).await.is_err() {
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

use rustproxy_proto::message::ControlMessage;

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
