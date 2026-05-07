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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSection {
    pub enable: bool,
    pub bind_addr: String,
    pub bind_port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsSection {
    pub auto_cert: bool,
    pub cert_file: String,
    pub key_file: String,
}

/// 客户端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client: ClientSection,
    pub proxy: Vec<ProxyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSection {
    pub server_addr: String,
    pub server_port: u16,
    pub token: String,
}

/// 代理规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRule {
    pub name: String,
    #[serde(rename = "type")]
    pub proxy_type: ProxyType,
    pub local_ip: String,
    pub local_port: u16,
    #[serde(default)]
    pub remote_port: u16,
    #[serde(default)]
    pub custom_domains: Vec<String>,
}

/// 代理协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Tcp,
    Udp,
    Http,
    Https,
}

impl std::fmt::Display for ProxyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyType::Tcp => write!(f, "tcp"),
            ProxyType::Udp => write!(f, "udp"),
            ProxyType::Http => write!(f, "http"),
            ProxyType::Https => write!(f, "https"),
        }
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
        assert_eq!(config.server.bind_port, 7000);
        assert_eq!(config.web.user, "admin");
        assert!(config.tls.auto_cert);
    }

    #[test]
    fn test_parse_client_config() {
        let content = r#"
[client]
server_addr = "127.0.0.1"
server_port = 7000
token = "test-token"

[[proxy]]
name = "ssh"
type = "tcp"
local_ip = "127.0.0.1"
local_port = 22
remote_port = 6000

[[proxy]]
name = "web"
type = "http"
local_ip = "127.0.0.1"
local_port = 8080
custom_domains = ["web.example.com"]
"#;
        let config = parse_client_config(content).unwrap();
        assert_eq!(config.client.server_port, 7000);
        assert_eq!(config.proxy.len(), 2);
        assert_eq!(config.proxy[0].proxy_type, ProxyType::Tcp);
        assert_eq!(config.proxy[1].proxy_type, ProxyType::Http);
    }
}
