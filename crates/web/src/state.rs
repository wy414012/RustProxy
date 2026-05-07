//! Web 应用共享状态
//!
//! 持有代理管理器和客户端通知回调，供 API 层读写代理规则、向客户端推送消息。

use std::sync::Arc;

use tokio::sync::RwLock;

use rustproxy_core::config::ServerConfig;
use rustproxy_core::proxy_manager::ProxyManager;

/// 向客户端发送消息的回调类型
/// 参数: (client_id, message_json)
pub type NotifyClientFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// 应用共享状态（线程安全）
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    server_config: RwLock<ServerConfig>,
    proxy_manager: ProxyManager,
    notify_client: RwLock<Option<NotifyClientFn>>,
    connected_clients: RwLock<Vec<String>>,
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
                connected_clients: RwLock::new(Vec::new()),
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
                connected_clients: RwLock::new(Vec::new()),
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
    pub async fn notify_client(&self, client_id: &str, message_json: &str) -> bool {
        let notify = self.inner.notify_client.read().await;
        if let Some(f) = notify.as_ref() {
            f(client_id, message_json)
        } else {
            false
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
}
