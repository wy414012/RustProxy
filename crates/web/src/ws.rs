//! WebSocket 实时状态推送
//!
//! 前端通过 WebSocket 连接获取代理规则和客户端状态的实时更新。
//! WebSocket 认证使用 JWT Token（通过查询参数传递）。

use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use serde::Deserialize;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsParams {
    pub token: Option<String>,
}

/// WebSocket 升级处理
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 验证 JWT Token
    if let Some(ref token) = params.token {
        if !validate_ws_token(&state, token).await {
            return (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
    } else {
        return (axum::http::StatusCode::UNAUTHORIZED, "Missing token").into_response();
    }

    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

/// 验证 WebSocket 连接的 JWT Token
async fn validate_ws_token(state: &AppState, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let config = state.server_config().await;
    let secret = config.server.token.clone();
    drop(config);
    crate::auth::validate_jwt(token, &secret).is_ok()
}

/// WebSocket 连接处理循环
async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(3));

    loop {
        interval.tick().await;

        let status_data = build_status_message(&state).await;

        match socket.send(Message::Text(status_data)).await {
            Ok(()) => {}
            Err(_) => break,
        }
    }
}

/// 构建状态推送消息
async fn build_status_message(state: &AppState) -> String {
    let proxies = state.proxy_manager().list_proxies().await;
    let clients = state.connected_clients().await;

    let proxy_summaries: Vec<serde_json::Value> = proxies
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.rule.name,
                "status": p.status.to_string(),
                "connections": p.connections,
                "bandwidth_in": p.bandwidth_in,
                "bandwidth_out": p.bandwidth_out,
            })
        })
        .collect();

    let msg = serde_json::json!({
        "type": "status",
        "proxies": proxy_summaries,
        "online_clients": clients.len(),
    });

    msg.to_string()
}
