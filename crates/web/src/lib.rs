//! RustProxy Web — 嵌入式 Web 管理面板
//!
//! 提供代理规则的增删查改、客户端状态监控、服务端状态查询等管理功能。

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
