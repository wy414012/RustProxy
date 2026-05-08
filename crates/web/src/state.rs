//! Web 应用共享状态
//!
//! 持有代理管理器和客户端通知回调，供 API 层读写代理规则、向客户端推送消息。
//! 同时包含登录速率限制状态，防止暴力破解。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use rustproxy_core::config::{ProxyRule, ServerConfig};
use rustproxy_core::proxy_manager::ProxyManager;

/// 向客户端发送消息的异步回调类型
/// 参数: (client_id, message_json) -> 是否发送成功
pub type NotifyClientFn = Arc<dyn Fn(String, String) -> bool + Send + Sync>;

/// 代理规则创建回调（异步，可等待监听器实际启动）
pub type OnProxyCreateFn =
    Arc<dyn Fn(ProxyRule) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// 代理规则删除回调（异步，可等待监听器实际停止）
pub type OnProxyDeleteFn =
    Arc<dyn Fn(ProxyRule) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// 登录速率限制：最大尝试次数
const MAX_LOGIN_ATTEMPTS: u32 = 5;
/// 登录速率限制：锁定时长（秒）
const LOGIN_LOCKOUT_SECS: u64 = 300;

/// 登录尝试记录（用于速率限制）
struct LoginAttempt {
    count: u32,
    window_start: Instant,
}

/// 应用共享状态（线程安全）
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    server_config: RwLock<ServerConfig>,
    proxy_manager: ProxyManager,
    notify_client: RwLock<Option<NotifyClientFn>>,
    on_proxy_create: RwLock<Option<OnProxyCreateFn>>,
    on_proxy_delete: RwLock<Option<OnProxyDeleteFn>>,
    connected_clients: RwLock<Vec<String>>,
    login_attempts: RwLock<HashMap<String, LoginAttempt>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish()
    }
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(config: ServerConfig) -> Self {
        let proxy_manager = ProxyManager::new();
        Self {
            inner: Arc::new(AppStateInner {
                server_config: RwLock::new(config),
                proxy_manager,
                notify_client: RwLock::new(None),
                on_proxy_create: RwLock::new(None),
                on_proxy_delete: RwLock::new(None),
                connected_clients: RwLock::new(Vec::new()),
                login_attempts: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// 创建带数据库的应用状态
    pub fn with_db(config: ServerConfig, db_path: &str) -> anyhow::Result<Self> {
        let proxy_manager = ProxyManager::open(db_path)?;
        Ok(Self {
            inner: Arc::new(AppStateInner {
                server_config: RwLock::new(config),
                proxy_manager,
                notify_client: RwLock::new(None),
                on_proxy_create: RwLock::new(None),
                on_proxy_delete: RwLock::new(None),
                connected_clients: RwLock::new(Vec::new()),
                login_attempts: RwLock::new(HashMap::new()),
            }),
        })
    }

    /// 获取代理管理器
    pub fn proxy_manager(&self) -> ProxyManager {
        self.inner.proxy_manager.clone()
    }

    /// 设置客户端通知回调
    pub async fn set_notify_client(&self, f: NotifyClientFn) {
        let mut notify = self.inner.notify_client.write().await;
        *notify = Some(f);
    }

    /// 通知客户端
    ///
    /// 使用 `tokio::spawn` 在独立任务中执行回调，避免在异步上下文中 `block_on` 死锁。
    /// 回调本身是同步的（内部可能用 `block_on`），所以必须 spawn 到阻塞线程池。
    pub async fn notify_client(&self, client_id: &str, message_json: &str) -> bool {
        // 先 clone 回调出来，避免持有锁跨 await
        let callback = {
            let notify = self.inner.notify_client.read().await;
            notify.clone()
        };

        if let Some(f) = callback {
            let cid = client_id.to_string();
            let msg = message_json.to_string();
            // 在独立任务中执行同步回调，避免阻塞当前 async 上下文
            tokio::task::spawn_blocking(move || f(cid, msg))
                .await
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// 设置代理规则创建回调
    pub async fn set_on_proxy_create(&self, f: OnProxyCreateFn) {
        let mut cb = self.inner.on_proxy_create.write().await;
        *cb = Some(f);
    }

    /// 触发代理规则创建回调（等待监听器实际启动完成）
    pub async fn on_proxy_create(&self, rule: &ProxyRule) {
        let callback = {
            let cb = self.inner.on_proxy_create.read().await;
            cb.clone()
        };
        if let Some(f) = callback {
            f(rule.clone()).await;
        }
    }

    /// 设置代理规则删除回调
    pub async fn set_on_proxy_delete(&self, f: OnProxyDeleteFn) {
        let mut cb = self.inner.on_proxy_delete.write().await;
        *cb = Some(f);
    }

    /// 触发代理规则删除回调（等待监听器实际停止完成）
    pub async fn on_proxy_delete(&self, rule: &ProxyRule) {
        let callback = {
            let cb = self.inner.on_proxy_delete.read().await;
            cb.clone()
        };
        if let Some(f) = callback {
            f(rule.clone()).await;
        }
    }

    /// 更新已连接客户端列表
    pub async fn set_connected_clients(&self, clients: Vec<String>) {
        let mut list = self.inner.connected_clients.write().await;
        *list = clients;
    }

    /// 获取已连接客户端列表
    pub async fn connected_clients(&self) -> Vec<String> {
        let list = self.inner.connected_clients.read().await;
        list.clone()
    }

    /// 获取服务端配置
    pub async fn server_config(&self) -> tokio::sync::RwLockReadGuard<'_, ServerConfig> {
        self.inner.server_config.read().await
    }

    /// 检查登录速率限制（返回 true 表示允许尝试）
    pub async fn check_login_rate_limit(&self, username: &str) -> bool {
        let mut attempts = self.inner.login_attempts.write().await;
        let now = Instant::now();

        if let Some(attempt) = attempts.get_mut(username) {
            let elapsed = now.duration_since(attempt.window_start);

            // 锁定期已过，重置
            if attempt.count >= MAX_LOGIN_ATTEMPTS && elapsed.as_secs() > LOGIN_LOCKOUT_SECS {
                attempt.count = 0;
                attempt.window_start = now;
                return true;
            }

            // 已超过最大尝试次数，仍在锁定期
            if attempt.count >= MAX_LOGIN_ATTEMPTS {
                return false;
            }

            true
        } else {
            true
        }
    }

    /// 记录登录尝试结果
    pub async fn record_login_attempt(&self, username: &str, success: bool) {
        let mut attempts = self.inner.login_attempts.write().await;
        let now = Instant::now();

        if success {
            // 登录成功，清除该用户的尝试记录
            attempts.remove(username);
        } else {
            // 登录失败，增加计数
            let attempt = attempts
                .entry(username.to_string())
                .or_insert(LoginAttempt {
                    count: 0,
                    window_start: now,
                });
            attempt.count += 1;
        }
    }
}
