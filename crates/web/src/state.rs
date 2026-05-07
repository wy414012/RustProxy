//! Web 应用共享状态

use std::sync::Arc;

/// 应用共享状态（线程安全）
#[derive(Debug, Clone, Default)]
pub struct AppState {
    #[allow(dead_code)] // TODO: 添加代理管理器、统计信息后移除
    inner: Arc<AppStateInner>,
}

#[derive(Debug, Default)]
struct AppStateInner {
    // TODO: 添加代理管理器、统计信息等
    // proxy_manager: ProxyManager,
    // stats: StatsCollector,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new() -> Self {
        Self::default()
    }
}
