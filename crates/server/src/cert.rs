//! TLS 证书配置构建

use std::sync::Arc;

use rustls::ServerConfig;
use rustproxy_core::config::TlsSection;
use rustproxy_core::tls;

/// 构建服务端 TLS 配置
pub fn build_server_tls_config(
    tls_config: &TlsSection,
) -> Result<Arc<ServerConfig>, anyhow::Error> {
    let (certs, key) = tls::get_or_create_cert(
        tls_config.auto_cert,
        &tls_config.cert_file,
        &tls_config.key_file,
    )?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow::anyhow!("TLS 配置失败: {}", e))?;

    Ok(Arc::new(config))
}
