//! Web API 路由定义

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use rustproxy_core::config::{ProxyRule, ProxyType};
use rustproxy_core::proxy_manager::ProxyEntry;

use crate::auth::generate_jwt;
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
// 输入验证
// ============================================================

/// 验证代理规则名称（仅允许字母、数字、横线、下划线，1-64字符）
fn validate_proxy_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("代理规则名称长度需在 1-64 之间".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("代理规则名称仅允许字母、数字、横线和下划线".into());
    }
    Ok(())
}

/// 验证 IP 地址格式
fn validate_ip(ip: &str) -> Result<(), String> {
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Err(format!("无效的 IP 地址格式: {}", ip));
    }
    Ok(())
}

/// 验证端口号（非零）
fn validate_port(port: u16, field_name: &str) -> Result<(), String> {
    if port == 0 {
        return Err(format!("{} 不能为 0", field_name));
    }
    Ok(())
}

/// 验证域名格式
fn validate_domain(domain: &str) -> Result<(), String> {
    if domain.is_empty() || domain.len() > 253 {
        return Err(format!("无效的域名长度: {}", domain));
    }
    if !domain
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '_')
    {
        return Err(format!("域名包含非法字符: {}", domain));
    }
    if domain.starts_with('-') || domain.starts_with('.') {
        return Err(format!("域名格式无效: {}", domain));
    }
    Ok(())
}

/// 验证 proxy_protocol 值（仅允许空/"v1"/"v2"）
fn validate_proxy_protocol(protocol: &str) -> Result<(), String> {
    match protocol {
        "" | "v1" | "v2" => Ok(()),
        _ => Err(format!(
            "无效的 proxy_protocol: {}，仅支持空/v1/v2",
            protocol
        )),
    }
}

/// 验证客户端 ID 格式
fn validate_client_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("客户端 ID 长度需在 1-64 之间".into());
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("客户端 ID 仅允许字母、数字、横线、下划线和点号".into());
    }
    Ok(())
}

/// 验证密码（支持明文和 bcrypt 哈希）
fn verify_password(provided: &str, stored: &str) -> bool {
    if stored.starts_with("$2b$") || stored.starts_with("$2a$") {
        // bcrypt 哈希验证
        bcrypt::verify(provided, stored).unwrap_or(false)
    } else {
        // 明文密码比较（向后兼容，使用常量时间比较防止时序攻击）
        tracing::warn!(
            "密码以明文存储，建议使用 bcrypt 哈希。运行 'rustproxy-server hash-password <密码>' 生成哈希后填入配置文件"
        );
        constant_time_eq(provided.as_bytes(), stored.as_bytes())
    }
}

/// 常量时间字节比较，防止时序攻击
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
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
    proxy_protocol: String,
    status: String,
    connections: u64,
    bandwidth_in: f64,
    bandwidth_out: f64,
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
            proxy_protocol: e.rule.proxy_protocol,
            status: e.status.to_string(),
            connections: e.connections,
            bandwidth_in: e.bandwidth_in,
            bandwidth_out: e.bandwidth_out,
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
    #[serde(default)]
    proxy_protocol: String,
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
    proxy_protocol: Option<String>,
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
    /// Token 有效期（秒）
    expires_in: u64,
}

// ============================================================
// API 处理函数
// ============================================================

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Json<ApiResponse<LoginResponse>> {
    // 1. 检查速率限制
    if !state.check_login_rate_limit(&req.username).await {
        return error_response(429, "登录尝试过于频繁，请稍后再试".into());
    }

    // 2. 验证用户名密码
    let config = state.server_config().await;
    let user_ok =
        req.username == config.web.user && verify_password(&req.password, &config.web.password);
    let token_expire_hours = config.web.token_expire_hours;
    let jwt_secret = config.web.jwt_secret.clone();
    drop(config);

    if !user_ok {
        state.record_login_attempt(&req.username, false).await;
        return error_response(401, "用户名或密码错误".into());
    }

    // 3. 重置速率限制
    state.record_login_attempt(&req.username, true).await;

    // 4. 生成 JWT Token
    let token = match generate_jwt(&req.username, &jwt_secret, token_expire_hours) {
        Ok(t) => t,
        Err(e) => return error_response(500, e),
    };

    ApiResponse::success(LoginResponse {
        token,
        expires_in: token_expire_hours * 3600,
    })
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
    // 输入验证
    if let Err(e) = validate_proxy_name(&payload.name) {
        return error_response(400, e);
    }
    if let Err(e) = validate_client_id(&payload.client_id) {
        return error_response(400, e);
    }
    if let Err(e) = validate_ip(&payload.local_ip) {
        return error_response(400, e);
    }
    if let Err(e) = validate_port(payload.local_port, "local_port") {
        return error_response(400, e);
    }
    if payload.proxy_type == "tcp" || payload.proxy_type == "udp" {
        if let Err(e) = validate_port(payload.remote_port, "remote_port") {
            return error_response(400, e);
        }
    }
    for domain in &payload.custom_domains {
        if let Err(e) = validate_domain(domain) {
            return error_response(400, e);
        }
    }
    if let Err(e) = validate_proxy_protocol(&payload.proxy_protocol) {
        return error_response(400, e);
    }

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
        proxy_protocol: payload.proxy_protocol,
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
        proxy_protocol: rule.proxy_protocol.clone(),
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
    // 输入验证（仅验证提供的字段）
    if let Some(ref t) = payload.proxy_type {
        if !matches!(t.as_str(), "tcp" | "udp" | "http" | "https") {
            return error_response(400, format!("不支持的代理类型: {}", t));
        }
    }
    if let Some(ref id) = payload.client_id {
        if let Err(e) = validate_client_id(id) {
            return error_response(400, e);
        }
    }
    if let Some(ref ip) = payload.local_ip {
        if let Err(e) = validate_ip(ip) {
            return error_response(400, e);
        }
    }
    if let Some(port) = payload.local_port {
        if let Err(e) = validate_port(port, "local_port") {
            return error_response(400, e);
        }
    }
    if let Some(port) = payload.remote_port {
        if let Err(e) = validate_port(port, "remote_port") {
            return error_response(400, e);
        }
    }
    if let Some(ref domains) = payload.custom_domains {
        for domain in domains {
            if let Err(e) = validate_domain(domain) {
                return error_response(400, e);
            }
        }
    }
    if let Some(ref protocol) = payload.proxy_protocol {
        if let Err(e) = validate_proxy_protocol(protocol) {
            return error_response(400, e);
        }
    }

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
        proxy_protocol: payload.proxy_protocol.unwrap_or(existing.proxy_protocol),
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
        proxy_protocol: updated_rule.proxy_protocol.clone(),
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
