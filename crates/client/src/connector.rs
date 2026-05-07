//! 客户端连接器
//!
//! 负责与服务端建立 TLS 隧道、认证、消息循环。

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;
use tokio_util::codec::Framed;

use rustproxy_core::config::ClientConfig;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::message::{AuthRequest, AuthResponse, ControlMessage, Message};

use crate::proxy_worker::ProxyWorkerManager;

/// 连接服务端并进入消息循环
pub async fn connect_and_run(config: &ClientConfig) -> anyhow::Result<()> {
    // 1. 建立 TCP 连接
    let addr = format!(
        "{}:{}",
        config.client.server_addr, config.client.server_port
    );
    tracing::info!("正在连接服务端: {}", addr);
    let tcp_stream = TcpStream::connect(&addr).await?;

    // 2. 建立 TLS 连接
    let tls_config = build_client_tls_config(&config.client.ca_cert)?;
    let connector = tokio_rustls::TlsConnector::from(tls_config);
    let domain = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| anyhow::anyhow!("域名解析失败: {}", e))?;
    let tls_stream = connector.connect(domain, tcp_stream).await?;
    tracing::info!("TLS 连接已建立");

    // 3. 创建 Framed 编解码
    let mut framed = Framed::new(tls_stream, FrameCodec);

    // 4. 发送认证请求
    let auth_req = AuthRequest {
        client_id: config.client.id.clone(),
        token: config.client.token.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    framed
        .send(Message::Control(ControlMessage::Auth(auth_req)))
        .await?;

    // 5. 等待认证响应
    let auth_resp = wait_auth_response(&mut framed).await?;
    if !auth_resp.success {
        anyhow::bail!("认证失败: {}", auth_resp.message);
    }
    tracing::info!("认证成功，服务端版本: {}", auth_resp.server_version);

    // 6. 创建代理工作管理器
    let proxy_manager = ProxyWorkerManager::new();
    let (_work_tx, mut work_rx) = mpsc::channel::<Message>(256);

    // 7. 心跳定时器
    let ping_interval = tokio::time::interval(std::time::Duration::from_secs(10));
    tokio::pin!(ping_interval);

    // 8. 进入消息循环
    loop {
        tokio::select! {
            // 接收服务端消息
            result = framed.next() => {
                match result {
                    Some(Ok(Message::Control(ctrl))) => {
                        match ctrl {
                            ControlMessage::Pong => {
                                // 心跳响应，正常
                            }
                            ControlMessage::ServerAssignProxy(req) => {
                                tracing::info!(
                                    "收到代理规则: {} ({} -> {}:{}, remote_port={})",
                                    req.name, req.proxy_type, req.local_ip, req.local_port, req.remote_port
                                );
                                if let Err(e) = proxy_manager.start_proxy(req, config).await {
                                    tracing::error!("启动代理失败: {}", e);
                                }
                            }
                            ControlMessage::ServerCloseProxy(req) => {
                                tracing::info!("关闭代理规则: {}", req.name);
                                proxy_manager.stop_proxy(&req.name).await;
                            }
                            ControlMessage::NewWorkConn(req) => {
                                tracing::debug!("新建工作连接: {} (conn_id={})", req.proxy_name, req.conn_id);
                                let cfg = config.clone();
                                let proxy_name = req.proxy_name.clone();
                                let conn_id = req.conn_id;
                                tokio::spawn(async move {
                                    if let Err(e) = crate::proxy_worker::open_work_connection(&cfg, &proxy_name, conn_id).await {
                                        tracing::error!("工作连接失败: {}", e);
                                    }
                                });
                            }
                            _ => {
                                tracing::debug!("收到控制消息: {:?}", ctrl);
                            }
                        }
                    }
                    Some(Ok(Message::Data(data))) => {
                        tracing::debug!("收到数据消息: conn_id={}, {} bytes", data.conn_id, data.data.len());
                    }
                    Some(Err(e)) => {
                        tracing::warn!("消息解析错误: {}", e);
                        break;
                    }
                    None => {
                        tracing::info!("服务端关闭连接");
                        break;
                    }
                }
            }
            // 发送心跳
            _ = ping_interval.tick() => {
                if framed.send(Message::Control(ControlMessage::Ping)).await.is_err() {
                    tracing::warn!("发送心跳失败");
                    break;
                }
            }
            // 发送工作连接消息（暂未使用）
            msg = work_rx.recv() => {
                match msg {
                    Some(msg) => {
                        if framed.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // 清理代理工作
    proxy_manager.stop_all().await;

    Ok(())
}

/// 等待认证响应
async fn wait_auth_response(
    framed: &mut Framed<TlsStream<TcpStream>, FrameCodec>,
) -> anyhow::Result<AuthResponse> {
    use tokio::time::{timeout, Duration};

    match timeout(Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(Message::Control(ControlMessage::AuthResp(resp))))) => Ok(resp),
        Ok(Some(Ok(_))) => anyhow::bail!("期望认证响应，收到其他消息"),
        Ok(Some(Err(e))) => anyhow::bail!("认证响应解析错误: {}", e),
        Ok(None) => anyhow::bail!("服务端在认证前关闭连接"),
        Err(_) => anyhow::bail!("认证超时"),
    }
}

/// 构建客户端 TLS 配置
fn build_client_tls_config(ca_cert_path: &str) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    if ca_cert_path.is_empty() {
        // 信任自签证书模式
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();
        return Ok(Arc::new(config));
    }

    // 加载指定 CA 证书
    let mut root_store = rustls::RootCertStore::empty();
    let ca_pem = std::fs::read(ca_cert_path)
        .map_err(|e| anyhow::anyhow!("读取 CA 证书失败 {}: {}", ca_cert_path, e))?;
    let ca_certs = rustls_pemfile::certs(&mut &ca_pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("解析 CA 证书失败: {}", e))?;
    for cert in ca_certs {
        root_store.add(cert)?;
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// 不验证证书（仅用于 auto_cert 自签模式）
#[derive(Debug)]
pub struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}
