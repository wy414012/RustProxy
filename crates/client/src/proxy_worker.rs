//! 代理工作管理
//!
//! 管理客户端本地的代理工作进程：接收服务端指令，建立本地连接，转发数据。

use std::collections::HashMap;
use std::sync::Arc;

use futures::SinkExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use rustproxy_core::config::ClientConfig;
use rustproxy_proto::message::ServerAssignProxyRequest;

/// 代理工作管理器
#[derive(Debug, Clone)]
pub struct ProxyWorkerManager {
    inner: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
}

impl ProxyWorkerManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动代理工作进程
    pub async fn start_proxy(
        &self,
        req: ServerAssignProxyRequest,
        config: &ClientConfig,
    ) -> anyhow::Result<()> {
        let name = req.name.clone();
        let local_addr = format!("{}:{}", req.local_ip, req.local_port);
        let proxy_type = req.proxy_type.clone();
        let config = config.clone();

        let handle = tokio::spawn(async move {
            match proxy_type.as_str() {
                "tcp" => run_tcp_proxy(&name, &local_addr, &config).await,
                "udp" => run_udp_proxy(&name, &local_addr, &config).await,
                "http" => run_http_proxy(&name, &local_addr, &config).await,
                "https" => run_https_proxy(&name, &local_addr, &config).await,
                _ => tracing::error!("不支持的代理类型: {}", proxy_type),
            }
        });

        let mut inner = self.inner.write().await;
        inner.insert(req.name.clone(), handle);
        Ok(())
    }

    /// 停止代理工作进程
    pub async fn stop_proxy(&self, name: &str) {
        let mut inner = self.inner.write().await;
        if let Some(handle) = inner.remove(name) {
            handle.abort();
            tracing::info!("代理 {} 已停止", name);
        }
    }

    /// 停止所有代理工作进程
    pub async fn stop_all(&self) {
        let mut inner = self.inner.write().await;
        for (name, handle) in inner.drain() {
            handle.abort();
            tracing::debug!("代理 {} 已停止", name);
        }
    }
}

/// TCP 代理：客户端收到 NewWorkConn 后，建立到本地服务的 TCP 连接并双向转发
async fn run_tcp_proxy(name: &str, local_addr: &str, _config: &ClientConfig) {
    tracing::info!("TCP 代理 {} 启动，目标: {}", name, local_addr);
    // 实际的数据转发在 open_work_connection 中处理
    // 这里保持任务存活，等待 NewWorkConn 指令
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// UDP 代理
async fn run_udp_proxy(name: &str, local_addr: &str, _config: &ClientConfig) {
    tracing::info!("UDP 代理 {} 启动，目标: {}", name, local_addr);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// HTTP 代理
async fn run_http_proxy(name: &str, local_addr: &str, _config: &ClientConfig) {
    tracing::info!("HTTP 代理 {} 启动，目标: {}", name, local_addr);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// HTTPS 代理
async fn run_https_proxy(name: &str, local_addr: &str, _config: &ClientConfig) {
    tracing::info!("HTTPS 代理 {} 启动，目标: {}", name, local_addr);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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
) -> anyhow::Result<()> {
    // 1. 建立新的 TLS 连接到服务端
    let addr = format!(
        "{}:{}",
        config.client.server_addr, config.client.server_port
    );
    let tcp_stream = tokio::net::TcpStream::connect(&addr).await?;

    let tls_config = build_work_tls_config(&config.client.ca_cert)?;
    let connector = tokio_rustls::TlsConnector::from(tls_config);
    let domain = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| anyhow::anyhow!("域名解析失败: {}", e))?;
    let tls_stream = connector.connect(domain, tcp_stream).await?;

    // 2. 创建 Framed 并发送确认
    let mut framed = tokio_util::codec::Framed::new(tls_stream, rustproxy_proto::frame::FrameCodec);

    let resp = rustproxy_proto::message::NewWorkConnResponse {
        proxy_name: proxy_name.to_string(),
        conn_id,
        success: true,
    };
    framed
        .send(rustproxy_proto::message::Message::Control(
            rustproxy_proto::message::ControlMessage::NewWorkConnResp(resp),
        ))
        .await?;

    // 3. 获取本地代理地址并连接
    // 从 proxy_manager 中查找，但这里简化为直接传入
    // TODO: 需要从 ProxyWorkerManager 获取 local_addr

    tracing::info!("工作连接已建立: proxy={}, conn_id={}", proxy_name, conn_id);

    // 4. 进入数据转发循环
    // 接收服务端发来的数据并转发到本地服务
    // 接收本地服务的数据并转发到服务端
    // 这部分将在后续迭代中完善

    Ok(())
}

fn build_work_tls_config(ca_cert_path: &str) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    // 复用 connector 中的 TLS 配置逻辑
    // 为简化代码，此处与 connector::build_client_tls_config 相同
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
