//! 配置解析模块

use serde::{Deserialize, Serialize};

/// 服务端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub web: WebSection,
    pub tls: TlsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    pub bind_addr: String,
    pub bind_port: u16,
    pub token: String,
    /// HTTP 代理监听端口（0 = 不监听）
    #[serde(default)]
    pub http_port: u16,
    /// HTTPS 代理监听端口（0 = 不监听）
    #[serde(default)]
    pub https_port: u16,
}

/// 默认 JWT Token 过期时间（小时）
fn default_token_expire_hours() -> u64 {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSection {
    pub enable: bool,
    pub bind_addr: String,
    pub bind_port: u16,
    pub user: String,
    /// 管理面板密码（支持明文或 bcrypt 哈希，推荐使用 bcrypt 哈希）
    pub password: String,
    /// JWT 签名密钥，独立于客户端认证 Token (server.token)
    /// 若留空，启动时自动生成随机密钥并打印到日志
    #[serde(default)]
    pub jwt_secret: String,
    /// JWT Token 过期时间（小时），默认 24
    #[serde(default = "default_token_expire_hours")]
    pub token_expire_hours: u64,
    /// CORS 允许的域名列表，留空则仅允许同源访问
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsSection {
    pub auto_cert: bool,
    pub cert_file: String,
    pub key_file: String,
}

/// 客户端配置（极简：仅需连接信息，代理规则由服务端下发）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client: ClientSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSection {
    /// 客户端唯一标识，服务端通过此 ID 关联代理规则
    pub id: String,
    pub server_addr: String,
    pub server_port: u16,
    pub token: String,
    /// 服务端 CA 证书路径，留空则信任自签证书（auto_cert 模式）
    #[serde(default)]
    pub ca_cert: String,
    /// TLS SNI 域名，用于证书验证时匹配服务端证书的域名
    /// 留空则使用 server_addr 作为 SNI
    /// 使用正式 CA 证书时必须设置为证书绑定的域名（如 proxy.example.com）
    #[serde(default)]
    pub server_name: String,
}

/// 代理规则（由服务端管理，通过 Web 面板配置，SQLite 持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRule {
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: ProxyType,
    /// 代理规则所属的客户端 ID
    pub client_id: String,
    /// 客户端本地服务地址
    pub local_ip: String,
    pub local_port: u16,
    /// 服务端暴露的公网端口（TCP/UDP 有效）
    #[serde(default)]
    pub remote_port: u16,
    /// 自定义域名（HTTP/HTTPS 有效）
    #[serde(default)]
    pub custom_domains: Vec<String>,
    /// PROXY Protocol 版本: "" (关闭) / "v1" / "v2"
    #[serde(default)]
    pub proxy_protocol: String,
}

/// 代理协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Tcp,
    Udp,
    Http,
    Https,
}

impl ProxyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyType::Tcp => "tcp",
            ProxyType::Udp => "udp",
            ProxyType::Http => "http",
            ProxyType::Https => "https",
        }
    }
}

impl std::fmt::Display for ProxyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 从 TOML 字符串解析服务端配置
pub fn parse_server_config(content: &str) -> crate::Result<ServerConfig> {
    Ok(toml::from_str(content)?)
}

/// 从 TOML 字符串解析客户端配置
pub fn parse_client_config(content: &str) -> crate::Result<ClientConfig> {
    Ok(toml::from_str(content)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_server_config() {
        let content = r#"
[server]
bind_addr = "0.0.0.0"
bind_port = 7000
token = "test-token"
http_port = 8080
https_port = 8443

[web]
enable = true
bind_addr = "0.0.0.0"
bind_port = 7500
user = "admin"
password = "admin"
jwt_secret = "my-jwt-secret"

[tls]
auto_cert = true
cert_file = ""
key_file = ""
"#;
        let config = parse_server_config(content).unwrap();
        assert_eq!(config.server.bind_port, 7000);
        assert_eq!(config.server.http_port, 8080);
        assert_eq!(config.server.https_port, 8443);
        assert_eq!(config.web.user, "admin");
        assert_eq!(config.web.jwt_secret, "my-jwt-secret");
        assert!(config.tls.auto_cert);
    }

    #[test]
    fn test_parse_server_config_no_http() {
        let content = r#"
[server]
bind_addr = "0.0.0.0"
bind_port = 7000
token = "test-token"

[web]
enable = true
bind_addr = "0.0.0.0"
bind_port = 7500
user = "admin"
password = "admin"

[tls]
auto_cert = true
cert_file = ""
key_file = ""
"#;
        let config = parse_server_config(content).unwrap();
        assert_eq!(config.server.http_port, 0);
        assert_eq!(config.server.https_port, 0);
        // jwt_secret 默认为空，服务端启动时会自动生成
        assert!(config.web.jwt_secret.is_empty());
    }

    #[test]
    fn test_parse_client_config() {
        let content = r#"
[client]
id = "my-laptop"
server_addr = "127.0.0.1"
server_port = 7000
token = "test-token"
ca_cert = ""
"#;
        let config = parse_client_config(content).unwrap();
        assert_eq!(config.client.id, "my-laptop");
        assert_eq!(config.client.server_port, 7000);
        assert!(config.client.ca_cert.is_empty());
    }

    #[test]
    fn test_client_config_minimal() {
        let content = r#"
[client]
id = "my-server"
server_addr = "1.2.3.4"
server_port = 7000
token = "secret"
"#;
        let config = parse_client_config(content).unwrap();
        assert_eq!(config.client.id, "my-server");
        assert!(config.client.ca_cert.is_empty());
    }
}
