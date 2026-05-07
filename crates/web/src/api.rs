//! Web API 路由定义

use axum::{
    extract::State,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// 代理规则相关 API 路由
pub fn proxy_routes() -> Router<AppState> {
    Router::new()
        .route("/proxies", get(list_proxies).post(create_proxy))
        .route("/proxies/:name", get(get_proxy).delete(delete_proxy))
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
    Json(_payload): Json<CreateProxyRequest>,
) -> Json<ProxyInfo> {
    // TODO: 创建代理规则
    todo!()
}

async fn get_proxy(
    State(_state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ProxyInfo> {
    // TODO: 获取单个代理信息
    todo!()
}

async fn delete_proxy(
    State(_state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> &'static str {
    // TODO: 删除代理规则
    "OK"
}

#[derive(Deserialize)]
struct CreateProxyRequest {
    name: String,
    proxy_type: String,
    local_ip: String,
    local_port: u16,
    remote_port: u16,
    custom_domains: Vec<String>,
}
