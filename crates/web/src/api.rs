//! Web API 路由定义

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use rustproxy_core::config::{ProxyRule, ProxyType};
use rustproxy_core::proxy_manager::ProxyEntry;

use crate::state::AppState;

/// 代理规则相关 API 路由
///
/// WebSocket 路由 (`/ws`) 在 `lib.rs` 中单独注册，不在此处。
pub fn proxy_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(login))
        .route("/proxies", get(list_proxies).post(create_proxy))
        .route(
            "/proxies/:name",
            get(get_proxy).put(update_proxy).delete(delete_proxy),
        )
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

fn error_response<T: Serialize>(code: u16, message: String) -> Json<ApiResponse<T>> {
    Json(ApiResponse {
        code,
        message,
        data: None,
    })
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

#[derive(Deserialize)]
struct UpdateProxyRequest {
    #[serde(rename = "type")]
    proxy_type: Option<String>,
    client_id: Option<String>,
    local_ip: Option<String>,
    local_port: Option<u16>,
    remote_port: Option<u16>,
    custom_domains: Option<Vec<String>>,
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
        error_response(401, "用户名或密码错误".into())
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
            return error_response(400, format!("不支持的代理类型: {}", payload.proxy_type));
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
        return error_response(400, e);
    }

    // 触发代理规则创建回调（启动公网监听器）
    state.on_proxy_create(&rule).await;

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
    match mgr.get_proxy(&rule.name).await {
        Some(entry) => ApiResponse::success(ProxyInfo::from(entry)),
        None => error_response(500, "创建成功但查询失败".into()),
    }
}

async fn get_proxy(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<ProxyInfo>> {
    match state.proxy_manager().get_proxy(&name).await {
        Some(entry) => ApiResponse::success(ProxyInfo::from(entry)),
        None => error_response(404, format!("代理规则不存在: {}", name)),
    }
}

async fn update_proxy(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(payload): Json<UpdateProxyRequest>,
) -> Json<ApiResponse<ProxyInfo>> {
    let mgr = state.proxy_manager();

    // 获取现有规则
    let existing = match mgr.get_proxy(&name).await {
        Some(e) => e.rule,
        None => return error_response(404, format!("代理规则不存在: {}", name)),
    };

    // 合并更新
    let updated_rule = ProxyRule {
        name: name.clone(),
        proxy_type: payload
            .proxy_type
            .as_deref()
            .and_then(|s| match s {
                "tcp" => Some(ProxyType::Tcp),
                "udp" => Some(ProxyType::Udp),
                "http" => Some(ProxyType::Http),
                "https" => Some(ProxyType::Https),
                _ => None,
            })
            .unwrap_or(existing.proxy_type),
        client_id: payload.client_id.unwrap_or(existing.client_id),
        local_ip: payload.local_ip.unwrap_or(existing.local_ip),
        local_port: payload.local_port.unwrap_or(existing.local_port),
        remote_port: payload.remote_port.unwrap_or(existing.remote_port),
        custom_domains: payload.custom_domains.unwrap_or(existing.custom_domains),
    };

    if let Err(e) = mgr.update_proxy(&name, updated_rule.clone()).await {
        return error_response(400, e);
    }

    // 先删除旧的公网监听器
    state.on_proxy_delete(&updated_rule).await;
    // 再创建新的公网监听器
    state.on_proxy_create(&updated_rule).await;

    // 通知客户端更新代理规则
    let assign_msg = rustproxy_proto::message::ServerAssignProxyRequest {
        name: updated_rule.name.clone(),
        proxy_type: updated_rule.proxy_type.to_string(),
        local_ip: updated_rule.local_ip.clone(),
        local_port: updated_rule.local_port,
        remote_port: updated_rule.remote_port,
        custom_domains: updated_rule.custom_domains.clone(),
    };
    let msg_json = serde_json::to_string(
        &rustproxy_proto::message::ControlMessage::ServerAssignProxy(assign_msg),
    )
    .unwrap_or_default();
    let _ = state
        .notify_client(&updated_rule.client_id, &msg_json)
        .await;

    match mgr.get_proxy(&name).await {
        Some(entry) => ApiResponse::success(ProxyInfo::from(entry)),
        None => error_response(500, "更新成功但查询失败".into()),
    }
}

async fn delete_proxy(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<()>> {
    let mgr = state.proxy_manager();

    // 获取规则以便通知客户端和停止监听器
    let entry = match mgr.get_proxy(&name).await {
        Some(e) => e,
        None => return error_response(404, format!("代理规则不存在: {}", name)),
    };

    if let Err(e) = mgr.remove_proxy(&name).await {
        return error_response(400, e);
    }

    // 触发代理规则删除回调（停止公网监听器）
    state.on_proxy_delete(&entry.rule).await;

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
