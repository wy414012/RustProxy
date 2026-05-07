//! Web API 路由定义

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use rustproxy_core::config::{ProxyRule, ProxyType};
use rustproxy_core::proxy_manager::ProxyEntry;

use crate::state::AppState;

/// 代理规则相关 API 路由
pub fn proxy_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(login))
        .route("/proxies", get(list_proxies))
        .route("/proxies", axum::routing::post(create_proxy))
        .route("/proxies/:name", get(get_proxy))
        .route("/proxies/:name", axum::routing::delete(delete_proxy))
        .route("/clients", get(list_clients))
        .route("/status", get(server_status))
}

// ============================================================
// 统一响应格式
// ============================================================

#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    code: u16,
    message: String,
    data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    fn success(data: T) -> Json<Self> {
        Json(Self {
            code: 200,
            message: "ok".into(),
            data: Some(data),
        })
    }
}

// ============================================================
// 数据传输对象
// ============================================================

#[derive(Serialize)]
struct ProxyInfo {
    name: String,
    proxy_type: String,
    client_id: String,
    local_ip: String,
    local_port: u16,
    remote_port: u16,
    custom_domains: Vec<String>,
    status: String,
    connections: u64,
    bytes_in: u64,
    bytes_out: u64,
}

impl From<ProxyEntry> for ProxyInfo {
    fn from(e: ProxyEntry) -> Self {
        Self {
            name: e.rule.name,
            proxy_type: e.rule.proxy_type.to_string(),
            client_id: e.rule.client_id,
            local_ip: e.rule.local_ip,
            local_port: e.rule.local_port,
            remote_port: e.rule.remote_port,
            custom_domains: e.rule.custom_domains,
            status: e.status.to_string(),
            connections: e.connections,
            bytes_in: e.bytes_in,
            bytes_out: e.bytes_out,
        }
    }
}

#[derive(Deserialize)]
struct CreateProxyRequest {
    name: String,
    #[serde(rename = "type")]
    proxy_type: String,
    client_id: String,
    local_ip: String,
    local_port: u16,
    #[serde(default)]
    remote_port: u16,
    #[serde(default)]
    custom_domains: Vec<String>,
}

#[derive(Serialize)]
struct ClientInfo {
    id: String,
    online: bool,
}

#[derive(Serialize)]
struct StatusInfo {
    version: String,
    connected_clients: usize,
    total_proxies: usize,
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

// ============================================================
// API 处理函数
// ============================================================

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Json<ApiResponse<LoginResponse>> {
    let config = state.server_config().await;
    let user_ok = req.username == config.web.user && req.password == config.web.password;
    let token_prefix = config.server.token[..8].to_string();
    drop(config);

    if user_ok {
        let token = format!("{}-{}-{}", req.username, make_timestamp(), token_prefix);
        ApiResponse::success(LoginResponse { token })
    } else {
        Json(ApiResponse {
            code: 401,
            message: "用户名或密码错误".into(),
            data: None,
        })
    }
}

/// 简易时间戳
fn make_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}", dur.as_secs())
}

async fn list_proxies(State(state): State<AppState>) -> Json<ApiResponse<Vec<ProxyInfo>>> {
    let entries = state.proxy_manager().list_proxies().await;
    let list: Vec<ProxyInfo> = entries.into_iter().map(Into::into).collect();
    ApiResponse::success(list)
}

async fn create_proxy(
    State(state): State<AppState>,
    Json(payload): Json<CreateProxyRequest>,
) -> Json<ApiResponse<ProxyInfo>> {
    // 解析代理类型
    let proxy_type = match payload.proxy_type.as_str() {
        "tcp" => ProxyType::Tcp,
        "udp" => ProxyType::Udp,
        "http" => ProxyType::Http,
        "https" => ProxyType::Https,
        _ => {
            return Json(ApiResponse {
                code: 400,
                message: format!("不支持的代理类型: {}", payload.proxy_type),
                data: None,
            })
        }
    };

    let rule = ProxyRule {
        name: payload.name,
        proxy_type,
        client_id: payload.client_id,
        local_ip: payload.local_ip,
        local_port: payload.local_port,
        remote_port: payload.remote_port,
        custom_domains: payload.custom_domains,
    };

    let mgr = state.proxy_manager();
    if let Err(e) = mgr.add_proxy(rule.clone()).await {
        return Json(ApiResponse {
            code: 400,
            message: e,
            data: None,
        });
    }

    // 通知客户端新的代理规则
    let assign_msg = rustproxy_proto::message::ServerAssignProxyRequest {
        name: rule.name.clone(),
        proxy_type: rule.proxy_type.to_string(),
        local_ip: rule.local_ip.clone(),
        local_port: rule.local_port,
        remote_port: rule.remote_port,
        custom_domains: rule.custom_domains.clone(),
    };
    let msg_json = serde_json::to_string(
        &rustproxy_proto::message::ControlMessage::ServerAssignProxy(assign_msg),
    )
    .unwrap_or_default();
    let _ = state.notify_client(&rule.client_id, &msg_json).await;

    // 返回创建的代理信息
    let entry = mgr.get_proxy(&rule.name).await.unwrap();
    ApiResponse::success(ProxyInfo::from(entry))
}

async fn get_proxy(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<ProxyInfo>> {
    match state.proxy_manager().get_proxy(&name).await {
        Some(entry) => ApiResponse::success(ProxyInfo::from(entry)),
        None => Json(ApiResponse {
            code: 404,
            message: format!("代理规则不存在: {}", name),
            data: None,
        }),
    }
}

async fn delete_proxy(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<()>> {
    let mgr = state.proxy_manager();

    // 获取规则以便通知客户端
    let entry = match mgr.get_proxy(&name).await {
        Some(e) => e,
        None => {
            return Json(ApiResponse {
                code: 404,
                message: format!("代理规则不存在: {}", name),
                data: None,
            })
        }
    };

    if let Err(e) = mgr.remove_proxy(&name).await {
        return Json(ApiResponse {
            code: 400,
            message: e,
            data: None,
        });
    }

    // 通知客户端关闭代理
    let close_msg = rustproxy_proto::message::ServerCloseProxyRequest { name: name.clone() };
    let msg_json = serde_json::to_string(
        &rustproxy_proto::message::ControlMessage::ServerCloseProxy(close_msg),
    )
    .unwrap_or_default();
    let _ = state.notify_client(&entry.rule.client_id, &msg_json).await;

    ApiResponse::success(())
}

async fn list_clients(State(state): State<AppState>) -> Json<ApiResponse<Vec<ClientInfo>>> {
    let connected = state.connected_clients().await;
    let all_clients = state.proxy_manager().list_client_ids().await;

    let clients: Vec<ClientInfo> = all_clients
        .into_iter()
        .map(|id| ClientInfo {
            id: id.clone(),
            online: connected.contains(&id),
        })
        .collect();

    ApiResponse::success(clients)
}

async fn server_status(State(state): State<AppState>) -> Json<ApiResponse<StatusInfo>> {
    let clients = state.connected_clients().await;
    let proxies = state.proxy_manager().list_proxies().await;
    ApiResponse::success(StatusInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        connected_clients: clients.len(),
        total_proxies: proxies.len(),
    })
}
