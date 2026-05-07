//! RustProxy Web — 嵌入式 Web 管理面板
//!
//! 提供代理规则的增删查改、流量统计、状态监控等管理功能。
//! 前端资源编译进服务端二进制文件，无需额外部署。

pub mod api;
pub mod state;

use axum::{routing::get, Router};
use tower_http::cors::CorsLayer;

use crate::api::proxy_routes;
use crate::state::AppState;

/// 构建 Web 应用路由
pub fn build_app(state: AppState) -> Router {
    Router::new()
        .nest("/api", proxy_routes())
        .route("/health", get(health_check))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}
