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
