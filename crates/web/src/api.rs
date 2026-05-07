//! Web API 路由定义

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// 代理规则相关 API 路由
pub fn proxy_routes() -> Router<AppState> {
    Router::new()
        .route("/proxies", get(list_proxies))
        .route("/proxies", axum::routing::post(create_proxy))
        .route("/proxies/:name", get(get_proxy))
        .route("/proxies/:name", axum::routing::delete(delete_proxy))
}

#[derive(Serialize)]
struct ProxyInfo {
    name: String,
    proxy_type: String,
    local_addr: String,
    remote_port: u16,
    status: String,
    connections: u64,
    bytes_in: u64,
    bytes_out: u64,
}

async fn list_proxies(State(_state): State<AppState>) -> Json<Vec<ProxyInfo>> {
    // TODO: 从状态中获取代理列表
    Json(vec![])
}

async fn create_proxy(
    State(_state): State<AppState>,
    Json(payload): Json<CreateProxyRequest>,
) -> Json<ProxyInfo> {
    let _ = payload; // TODO: 实现创建代理逻辑
    todo!()
}

async fn get_proxy(
    State(_state): State<AppState>,
    axum::extract::Path(_name): axum::extract::Path<String>,
) -> Json<ProxyInfo> {
    // TODO: 获取单个代理信息
    todo!()
}

async fn delete_proxy(
    State(_state): State<AppState>,
    axum::extract::Path(_name): axum::extract::Path<String>,
) -> &'static str {
    // TODO: 删除代理规则
    "OK"
}

#[derive(Deserialize)]
struct CreateProxyRequest {
    #[allow(dead_code)] // TODO: 实现 create_proxy 时移除
    name: String,
    #[allow(dead_code)]
    proxy_type: String,
    #[allow(dead_code)]
    local_ip: String,
    #[allow(dead_code)]
    local_port: u16,
    #[allow(dead_code)]
    remote_port: u16,
    #[allow(dead_code)]
    custom_domains: Vec<String>,
}
