//! Web 应用共享状态

use std::sync::Arc;

/// 应用共享状态（线程安全）
#[derive(Debug, Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

#[derive(Debug)]
struct AppStateInner {
    // TODO: 添加代理管理器、统计信息等
    // proxy_manager: ProxyManager,
    // stats: StatsCollector,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AppStateInner {}),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
