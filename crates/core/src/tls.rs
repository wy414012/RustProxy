//! TLS 证书管理模块
//!
//! 支持自动生成自签证书（auto_cert）和加载用户指定证书。

use std::path::Path;

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::error::{Error, Result};

/// 生成自签 TLS 证书和密钥
pub fn generate_self_signed_cert() -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>
{
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "RustProxy Server");
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "RustProxy");

    // 添加 SAN（Subject Alternative Names）以支持 IP 和域名
    params.subject_alt_names = vec![
        rcgen::SanType::DnsName(
            rcgen::Ia5String::try_from("localhost")
                .map_err(|e| Error::Tls(format!("DNS 名称解析失败: {}", e)))?,
        ),
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
        rcgen::SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    ];

    let key_pair = KeyPair::generate().map_err(|e| Error::Tls(format!("生成密钥对失败: {}", e)))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| Error::Tls(format!("自签证书失败: {}", e)))?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    Ok((vec![cert_der], key_der))
}

/// 从 PEM 文件加载证书和密钥
pub fn load_cert_from_files(
    cert_path: &str,
    key_path: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_pem = std::fs::read(cert_path)
        .map_err(|e| Error::Tls(format!("读取证书文件失败 {}: {}", cert_path, e)))?;
    let key_pem = std::fs::read(key_path)
        .map_err(|e| Error::Tls(format!("读取密钥文件失败 {}: {}", key_path, e)))?;

    let certs = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::Tls(format!("解析证书 PEM 失败: {}", e)))?;

    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .map_err(|e| Error::Tls(format!("解析密钥 PEM 失败: {}", e)))?
        .ok_or_else(|| Error::Tls("未找到有效私钥".to_string()))?;

    Ok((certs, key))
}

/// 保存证书和密钥到 PEM 文件
pub fn save_cert_to_files(
    certs: &[CertificateDer<'static>],
    key: &PrivateKeyDer<'static>,
    cert_path: &str,
    key_path: &str,
) -> Result<()> {
    use std::io::Write;

    let cert_dir = Path::new(cert_path).parent();
    if let Some(dir) = cert_dir {
        std::fs::create_dir_all(dir).map_err(|e| Error::Tls(format!("创建证书目录失败: {}", e)))?;
    }
    let key_dir = Path::new(key_path).parent();
    if let Some(dir) = key_dir {
        std::fs::create_dir_all(dir).map_err(|e| Error::Tls(format!("创建密钥目录失败: {}", e)))?;
    }

    let mut cert_file = std::fs::File::create(cert_path)
        .map_err(|e| Error::Tls(format!("创建证书文件失败: {}", e)))?;
    for cert in certs {
        let pem = pem::encode(&pem::Pem::new("CERTIFICATE", cert.as_ref()));
        cert_file
            .write_all(pem.as_bytes())
            .map_err(|e| Error::Tls(format!("写入证书文件失败: {}", e)))?;
    }

    let mut key_file = std::fs::File::create(key_path)
        .map_err(|e| Error::Tls(format!("创建密钥文件失败: {}", e)))?;
    let key_pem = match key {
        PrivateKeyDer::Pkcs1(data) => {
            pem::encode(&pem::Pem::new("RSA PRIVATE KEY", data.secret_pkcs1_der()))
        }
        PrivateKeyDer::Pkcs8(data) => {
            pem::encode(&pem::Pem::new("PRIVATE KEY", data.secret_pkcs8_der()))
        }
        PrivateKeyDer::Sec1(data) => {
            pem::encode(&pem::Pem::new("EC PRIVATE KEY", data.secret_sec1_der()))
        }
        _ => return Err(Error::Tls("不支持的密钥格式".to_string())),
    };
    key_file
        .write_all(key_pem.as_bytes())
        .map_err(|e| Error::Tls(format!("写入密钥文件失败: {}", e)))?;

    Ok(())
}

/// 获取或创建 TLS 证书
///
/// - 如果 cert_file/key_file 非空且文件存在，从文件加载
/// - 如果 auto_cert 且文件不存在，自动生成并保存
/// - 否则内存中生成（不持久化）
pub fn get_or_create_cert(
    auto_cert: bool,
    cert_path: &str,
    key_path: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    // 尝试从文件加载
    if !cert_path.is_empty()
        && !key_path.is_empty()
        && Path::new(cert_path).exists()
        && Path::new(key_path).exists()
    {
        tracing::info!("从文件加载 TLS 证书: {} / {}", cert_path, key_path);
        return load_cert_from_files(cert_path, key_path);
    }

    // 自动生成
    tracing::info!("自动生成自签 TLS 证书");
    let (certs, key) = generate_self_signed_cert()?;

    // 如果配置了路径，保存到文件以便后续重启复用
    if auto_cert && !cert_path.is_empty() && !key_path.is_empty() {
        if let Err(e) = save_cert_to_files(&certs, &key, cert_path, key_path) {
            tracing::warn!("保存证书文件失败（将仅使用内存证书）: {}", e);
        } else {
            tracing::info!("自签证书已保存: {} / {}", cert_path, key_path);
        }
    }

    Ok((certs, key))
}
