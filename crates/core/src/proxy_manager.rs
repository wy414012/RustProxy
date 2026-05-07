//! 代理管理器
//!
//! 管理所有代理规则：增删查改、实时带宽监控。
//! 使用 SQLite 持久化，服务端重启后规则不丢失。
//! 客户端每次启动从服务端拉取规则，无需本地存储。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rusqlite::params;
use tokio::sync::{Mutex, RwLock};

use crate::config::{ProxyRule, ProxyType};

/// 代理规则运行时状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Error,
}

impl std::fmt::Display for ProxyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyStatus::Starting => write!(f, "starting"),
            ProxyStatus::Running => write!(f, "running"),
            ProxyStatus::Stopping => write!(f, "stopping"),
            ProxyStatus::Stopped => write!(f, "stopped"),
            ProxyStatus::Error => write!(f, "error"),
        }
    }
}

/// 代理规则的运行时信息
#[derive(Debug, Clone)]
pub struct ProxyEntry {
    pub rule: ProxyRule,
    pub status: ProxyStatus,
    pub connections: u64,
    /// 入站实时带宽（字节/秒）
    pub bandwidth_in: f64,
    /// 出站实时带宽（字节/秒）
    pub bandwidth_out: f64,
}

/// 代理管理器（SQLite 持久化 + 内存运行时状态）
#[derive(Clone)]
pub struct ProxyManager {
    /// SQLite 连接（Mutex 保护，因为 rusqlite::Connection 不是 Sync）
    db: Arc<Mutex<rusqlite::Connection>>,
    /// 运行时状态（不持久化，重启后重置）
    runtime: Arc<RwLock<HashMap<String, RuntimeState>>>,
}

/// 运行时状态（仅存在于内存中）
#[derive(Debug, Clone)]
struct RuntimeState {
    status: ProxyStatus,
    connections: u64,
    /// 字节累加器（用于带宽计算）
    bytes_in: u64,
    bytes_out: u64,
    /// 上次采样时刻
    last_sample_instant: Option<Instant>,
    /// 上次采样时的字节计数
    last_sample_bytes_in: u64,
    last_sample_bytes_out: u64,
    /// 当前入站带宽（字节/秒）
    bandwidth_in: f64,
    /// 当前出站带宽（字节/秒）
    bandwidth_out: f64,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            status: ProxyStatus::Stopped,
            connections: 0,
            bytes_in: 0,
            bytes_out: 0,
            last_sample_instant: None,
            last_sample_bytes_in: 0,
            last_sample_bytes_out: 0,
            bandwidth_in: 0.0,
            bandwidth_out: 0.0,
        }
    }
}

impl std::fmt::Debug for ProxyManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyManager").finish()
    }
}

impl ProxyManager {
    /// 创建内存数据库的代理管理器（仅用于测试）
    pub fn new() -> Self {
        let db = rusqlite::Connection::open_in_memory().expect("创建内存数据库失败");
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS proxy_rules (
                name        TEXT PRIMARY KEY,
                proxy_type  TEXT NOT NULL,
                client_id   TEXT NOT NULL,
                local_ip    TEXT NOT NULL,
                local_port  INTEGER NOT NULL,
                remote_port INTEGER NOT NULL DEFAULT 0,
                custom_domains TEXT NOT NULL DEFAULT '[]',
                proxy_protocol TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_proxy_rules_client_id ON proxy_rules(client_id);
            ",
        )
        .expect("初始化数据库表失败");

        Self {
            db: Arc::new(Mutex::new(db)),
            runtime: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 打开 SQLite 数据库文件
    pub fn open(db_path: &str) -> anyhow::Result<Self> {
        // 确保数据库目录存在
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = rusqlite::Connection::open(db_path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS proxy_rules (
                name        TEXT PRIMARY KEY,
                proxy_type  TEXT NOT NULL,
                client_id   TEXT NOT NULL,
                local_ip    TEXT NOT NULL,
                local_port  INTEGER NOT NULL,
                remote_port INTEGER NOT NULL DEFAULT 0,
                custom_domains TEXT NOT NULL DEFAULT '[]',
                proxy_protocol TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_proxy_rules_client_id ON proxy_rules(client_id);

            -- 为旧版本数据库迁移添加 proxy_protocol 列
            ALTER TABLE proxy_rules ADD COLUMN proxy_protocol TEXT NOT NULL DEFAULT '';
            ",
        )
        // ALTER TABLE 在列已存在时会报错，忽略此错误
        .or_else(|_| -> anyhow::Result<()> { Ok(()) })?;

        tracing::info!("数据库已打开: {}", db_path);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            runtime: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// 添加代理规则（持久化到数据库）
    pub async fn add_proxy(&self, rule: ProxyRule) -> Result<(), String> {
        // 先检查内存中是否存在（快速路径）
        {
            let inner = self.runtime.read().await;
            if inner.contains_key(&rule.name) {
                return Err(format!("代理规则名称已存在: {}", rule.name));
            }
        }

        // 持久化到数据库
        let domains_json =
            serde_json::to_string(&rule.custom_domains).unwrap_or_else(|_| "[]".to_string());
        {
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO proxy_rules (name, proxy_type, client_id, local_ip, local_port, remote_port, custom_domains, proxy_protocol)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    rule.name,
                    rule.proxy_type.as_str(),
                    rule.client_id,
                    rule.local_ip,
                    rule.local_port,
                    rule.remote_port,
                    domains_json,
                    rule.proxy_protocol,
                ],
            )
            .map_err(|e| format!("数据库写入失败: {}", e))?;
        }

        // 更新内存状态
        {
            let mut inner = self.runtime.write().await;
            inner.insert(rule.name.clone(), RuntimeState::new());
        }

        tracing::info!(
            "代理规则已添加: {} ({}, client={})",
            rule.name,
            rule.proxy_type,
            rule.client_id
        );
        Ok(())
    }

    /// 删除代理规则（从数据库中移除）
    pub async fn remove_proxy(&self, name: &str) -> Result<ProxyRule, String> {
        // 先从数据库读取规则
        let rule = self
            .query_rule(name)
            .await
            .map_err(|e| format!("代理规则不存在: {}", e))?;

        // 从数据库删除
        {
            let db = self.db.lock().await;
            db.execute("DELETE FROM proxy_rules WHERE name = ?1", params![name])
                .map_err(|e| format!("数据库删除失败: {}", e))?;
        }

        // 更新内存状态
        {
            let mut inner = self.runtime.write().await;
            inner.remove(name);
        }

        tracing::info!("代理规则已删除: {}", name);
        Ok(rule)
    }

    /// 更新代理规则
    pub async fn update_proxy(&self, name: &str, new_rule: ProxyRule) -> Result<(), String> {
        let domains_json =
            serde_json::to_string(&new_rule.custom_domains).unwrap_or_else(|_| "[]".to_string());

        let rows = {
            let db = self.db.lock().await;
            db.execute(
                "UPDATE proxy_rules SET proxy_type=?2, client_id=?3, local_ip=?4, local_port=?5, remote_port=?6, custom_domains=?7, proxy_protocol=?8 WHERE name=?1",
                params![
                    name,
                    new_rule.proxy_type.as_str(),
                    new_rule.client_id,
                    new_rule.local_ip,
                    new_rule.local_port,
                    new_rule.remote_port,
                    domains_json,
                    new_rule.proxy_protocol,
                ],
            )
            .map_err(|e| format!("数据库更新失败: {}", e))?
        };

        if rows == 0 {
            return Err(format!("代理规则不存在: {}", name));
        }

        Ok(())
    }

    /// 获取代理规则（含运行时状态）
    pub async fn get_proxy(&self, name: &str) -> Option<ProxyEntry> {
        let rule = self.query_rule(name).await.ok()?;
        let inner = self.runtime.read().await;
        let rt = inner.get(name).cloned().unwrap_or_else(RuntimeState::new);
        Some(ProxyEntry {
            rule,
            status: rt.status,
            connections: rt.connections,
            bandwidth_in: rt.bandwidth_in,
            bandwidth_out: rt.bandwidth_out,
        })
    }

    /// 列出所有代理规则（含运行时状态）
    pub async fn list_proxies(&self) -> Vec<ProxyEntry> {
        let rules = self.query_all_rules().await;
        let inner = self.runtime.read().await;
        rules
            .into_iter()
            .map(|rule| {
                let rt = inner
                    .get(&rule.name)
                    .cloned()
                    .unwrap_or_else(RuntimeState::new);
                ProxyEntry {
                    rule,
                    status: rt.status,
                    connections: rt.connections,
                    bandwidth_in: rt.bandwidth_in,
                    bandwidth_out: rt.bandwidth_out,
                }
            })
            .collect()
    }

    /// 列出指定客户端的代理规则
    pub async fn list_proxies_by_client(&self, client_id: &str) -> Vec<ProxyEntry> {
        let rules = self.query_rules_by_client(client_id).await;
        let inner = self.runtime.read().await;
        rules
            .into_iter()
            .map(|rule| {
                let rt = inner
                    .get(&rule.name)
                    .cloned()
                    .unwrap_or_else(RuntimeState::new);
                ProxyEntry {
                    rule,
                    status: rt.status,
                    connections: rt.connections,
                    bandwidth_in: rt.bandwidth_in,
                    bandwidth_out: rt.bandwidth_out,
                }
            })
            .collect()
    }

    /// 获取所有已注册的客户端 ID 列表
    pub async fn list_client_ids(&self) -> Vec<String> {
        let db = self.db.lock().await;
        let mut stmt = match db.prepare("SELECT DISTINCT client_id FROM proxy_rules") {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let result = match stmt.query_map([], |row| row.get(0)) {
            Ok(rows) => rows
                .filter_map(|r: std::result::Result<String, _>| r.ok())
                .collect(),
            Err(_) => vec![],
        };
        result
    }

    /// 更新代理规则状态（仅内存）
    pub async fn update_status(&self, name: &str, status: ProxyStatus) {
        let mut inner = self.runtime.write().await;
        if let Some(rt) = inner.get_mut(name) {
            rt.status = status;
        }
    }

    /// 增加连接计数
    pub async fn inc_connections(&self, name: &str) {
        let mut inner = self.runtime.write().await;
        if let Some(rt) = inner.get_mut(name) {
            rt.connections += 1;
        }
    }

    /// 减少连接计数
    pub async fn dec_connections(&self, name: &str) {
        let mut inner = self.runtime.write().await;
        if let Some(rt) = inner.get_mut(name) {
            rt.connections = rt.connections.saturating_sub(1);
        }
    }

    /// 增加流量统计（累加字节计数，用于带宽计算）
    pub async fn add_traffic(&self, name: &str, bytes_in: u64, bytes_out: u64) {
        let mut inner = self.runtime.write().await;
        if let Some(rt) = inner.get_mut(name) {
            rt.bytes_in += bytes_in;
            rt.bytes_out += bytes_out;
        }
    }

    /// 更新所有代理的实时带宽（周期性调用，如每 2 秒）
    ///
    /// 原理：对比两次采样间的字节增量 / 时间间隔 = 当前带宽
    pub async fn update_bandwidth(&self) {
        let now = Instant::now();
        let mut inner = self.runtime.write().await;
        for rt in inner.values_mut() {
            if let Some(last_instant) = rt.last_sample_instant {
                let elapsed = now.duration_since(last_instant).as_secs_f64();
                if elapsed > 0.0 {
                    let delta_in = rt.bytes_in.saturating_sub(rt.last_sample_bytes_in);
                    let delta_out = rt.bytes_out.saturating_sub(rt.last_sample_bytes_out);
                    rt.bandwidth_in = delta_in as f64 / elapsed;
                    rt.bandwidth_out = delta_out as f64 / elapsed;
                }
            }
            rt.last_sample_instant = Some(now);
            rt.last_sample_bytes_in = rt.bytes_in;
            rt.last_sample_bytes_out = rt.bytes_out;
        }
    }

    /// 从数据库加载所有规则到内存运行时状态（服务端启动时调用）
    pub async fn load_from_db(&self) {
        let rules = self.query_all_rules().await;
        let mut inner = self.runtime.write().await;
        for rule in &rules {
            if !inner.contains_key(&rule.name) {
                inner.insert(rule.name.clone(), RuntimeState::new());
            }
        }
        tracing::info!("从数据库加载了 {} 条代理规则", rules.len());
    }

    // ============================================================
    // 数据库查询内部方法
    // ============================================================

    async fn query_rule(&self, name: &str) -> Result<ProxyRule, String> {
        let db = self.db.lock().await;
        let mut stmt = db
            .prepare("SELECT name, proxy_type, client_id, local_ip, local_port, remote_port, custom_domains, proxy_protocol FROM proxy_rules WHERE name = ?1")
            .map_err(|e| format!("数据库查询失败: {}", e))?;

        stmt.query_row(params![name], |row| {
            let name: String = row.get(0)?;
            let proxy_type_str: String = row.get(1)?;
            let client_id: String = row.get(2)?;
            let local_ip: String = row.get(3)?;
            let local_port: u16 = row.get(4)?;
            let remote_port: u16 = row.get(5)?;
            let domains_json: String = row.get(6)?;
            let proxy_protocol: String = row.get(7)?;

            let proxy_type = parse_proxy_type(&proxy_type_str);
            let custom_domains: Vec<String> =
                serde_json::from_str(&domains_json).unwrap_or_default();

            Ok(ProxyRule {
                name,
                proxy_type,
                client_id,
                local_ip,
                local_port,
                remote_port,
                custom_domains,
                proxy_protocol,
            })
        })
        .map_err(|e| format!("{}", e))
    }

    async fn query_all_rules(&self) -> Vec<ProxyRule> {
        let db = self.db.lock().await;
        let mut stmt = match db.prepare(
            "SELECT name, proxy_type, client_id, local_ip, local_port, remote_port, custom_domains, proxy_protocol FROM proxy_rules",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let rows = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let proxy_type_str: String = row.get(1)?;
            let client_id: String = row.get(2)?;
            let local_ip: String = row.get(3)?;
            let local_port: u16 = row.get(4)?;
            let remote_port: u16 = row.get(5)?;
            let domains_json: String = row.get(6)?;
            let proxy_protocol: String = row.get(7)?;

            let proxy_type = parse_proxy_type(&proxy_type_str);
            let custom_domains: Vec<String> =
                serde_json::from_str(&domains_json).unwrap_or_default();

            Ok(ProxyRule {
                name,
                proxy_type,
                client_id,
                local_ip,
                local_port,
                remote_port,
                custom_domains,
                proxy_protocol,
            })
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        rows.filter_map(|r| r.ok()).collect()
    }

    async fn query_rules_by_client(&self, client_id: &str) -> Vec<ProxyRule> {
        let db = self.db.lock().await;
        let mut stmt = match db.prepare(
            "SELECT name, proxy_type, client_id, local_ip, local_port, remote_port, custom_domains, proxy_protocol FROM proxy_rules WHERE client_id = ?1",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let rows = match stmt.query_map(params![client_id], |row| {
            let name: String = row.get(0)?;
            let proxy_type_str: String = row.get(1)?;
            let client_id: String = row.get(2)?;
            let local_ip: String = row.get(3)?;
            let local_port: u16 = row.get(4)?;
            let remote_port: u16 = row.get(5)?;
            let domains_json: String = row.get(6)?;
            let proxy_protocol: String = row.get(7)?;

            let proxy_type = parse_proxy_type(&proxy_type_str);
            let custom_domains: Vec<String> =
                serde_json::from_str(&domains_json).unwrap_or_default();

            Ok(ProxyRule {
                name,
                proxy_type,
                client_id,
                local_ip,
                local_port,
                remote_port,
                custom_domains,
                proxy_protocol,
            })
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        rows.filter_map(|r| r.ok()).collect()
    }
}

impl Default for ProxyManager {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_proxy_type(s: &str) -> ProxyType {
    match s {
        "tcp" => ProxyType::Tcp,
        "udp" => ProxyType::Udp,
        "http" => ProxyType::Http,
        "https" => ProxyType::Https,
        _ => ProxyType::Tcp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_proxy() {
        let mgr = ProxyManager::new();

        let rule = ProxyRule {
            name: "test-ssh".to_string(),
            proxy_type: ProxyType::Tcp,
            client_id: "my-laptop".to_string(),
            local_ip: "127.0.0.1".to_string(),
            local_port: 22,
            remote_port: 6000,
            custom_domains: vec![],
            proxy_protocol: String::new(),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            mgr.add_proxy(rule.clone()).await.unwrap();
            let entry = mgr.get_proxy("test-ssh").await.unwrap();
            assert_eq!(entry.rule.name, "test-ssh");
            assert_eq!(entry.rule.client_id, "my-laptop");
        });
    }

    #[test]
    fn test_list_by_client() {
        let mgr = ProxyManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            mgr.add_proxy(ProxyRule {
                name: "ssh".to_string(),
                proxy_type: ProxyType::Tcp,
                client_id: "client-a".to_string(),
                local_ip: "127.0.0.1".to_string(),
                local_port: 22,
                remote_port: 6000,
                custom_domains: vec![],
                proxy_protocol: String::new(),
            })
            .await
            .unwrap();

            mgr.add_proxy(ProxyRule {
                name: "web".to_string(),
                proxy_type: ProxyType::Http,
                client_id: "client-b".to_string(),
                local_ip: "127.0.0.1".to_string(),
                local_port: 80,
                remote_port: 0,
                custom_domains: vec!["example.com".to_string()],
                proxy_protocol: String::new(),
            })
            .await
            .unwrap();

            let list_a = mgr.list_proxies_by_client("client-a").await;
            assert_eq!(list_a.len(), 1);
            assert_eq!(list_a[0].rule.name, "ssh");

            let list_b = mgr.list_proxies_by_client("client-b").await;
            assert_eq!(list_b.len(), 1);
            assert_eq!(list_b[0].rule.custom_domains, vec!["example.com"]);
        });
    }

    #[test]
    fn test_remove_proxy() {
        let mgr = ProxyManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            mgr.add_proxy(ProxyRule {
                name: "test".to_string(),
                proxy_type: ProxyType::Tcp,
                client_id: "c1".to_string(),
                local_ip: "127.0.0.1".to_string(),
                local_port: 22,
                remote_port: 6000,
                custom_domains: vec![],
                proxy_protocol: String::new(),
            })
            .await
            .unwrap();

            let removed = mgr.remove_proxy("test").await.unwrap();
            assert_eq!(removed.name, "test");

            assert!(mgr.get_proxy("test").await.is_none());
        });
    }

    #[test]
    fn test_duplicate_name() {
        let mgr = ProxyManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            mgr.add_proxy(ProxyRule {
                name: "dup".to_string(),
                proxy_type: ProxyType::Tcp,
                client_id: "c1".to_string(),
                local_ip: "127.0.0.1".to_string(),
                local_port: 22,
                remote_port: 6000,
                custom_domains: vec![],
                proxy_protocol: String::new(),
            })
            .await
            .unwrap();

            let result = mgr
                .add_proxy(ProxyRule {
                    name: "dup".to_string(),
                    proxy_type: ProxyType::Udp,
                    client_id: "c2".to_string(),
                    local_ip: "127.0.0.1".to_string(),
                    local_port: 53,
                    remote_port: 6001,
                    custom_domains: vec![],
                    proxy_protocol: String::new(),
                })
                .await;
            assert!(result.is_err());
        });
    }

    #[tokio::test]
    async fn test_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_path_str = db_path.to_string_lossy().to_string();

        // 写入数据
        {
            let mgr = ProxyManager::open(&db_path_str).unwrap();
            mgr.add_proxy(ProxyRule {
                name: "persistent-rule".to_string(),
                proxy_type: ProxyType::Tcp,
                client_id: "test-client".to_string(),
                local_ip: "127.0.0.1".to_string(),
                local_port: 8080,
                remote_port: 9090,
                custom_domains: vec![],
                proxy_protocol: "v1".to_string(),
            })
            .await
            .unwrap();
        }

        // 重新打开数据库，数据应该还在
        {
            let mgr = ProxyManager::open(&db_path_str).unwrap();
            let proxies = mgr.list_proxies().await;
            assert_eq!(proxies.len(), 1);
            assert_eq!(proxies[0].rule.name, "persistent-rule");
            assert_eq!(proxies[0].rule.remote_port, 9090);
            assert_eq!(proxies[0].rule.proxy_protocol, "v1");
        }
    }
}
