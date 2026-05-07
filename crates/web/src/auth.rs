//! 认证中间件
//!
//! 保护 `/api/*` 路由（除登录接口外），验证 Bearer Token。

use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};

use crate::state::AppState;

/// API 认证中间件
///
/// 跳过 `/health`、`/api/auth/login` 和 WebSocket 握手路径，
/// 其余请求需携带有效的 Bearer Token。
pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, axum::http::StatusCode> {
    let path = req.uri().path();

    // 无需认证的路径
    if path == "/health"
        || path == "/api/auth/login"
        || path.starts_with("/api/ws")
        || !path.starts_with("/api/")
    {
        return Ok(next.run(req).await);
    }

    // 检查 Authorization 头
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if validate_token(&state, token).await {
                Ok(next.run(req).await)
            } else {
                Err(axum::http::StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(axum::http::StatusCode::UNAUTHORIZED),
    }
}

/// 验证 Token 有效性
///
/// Token 格式: `{username}-{timestamp}-{token_prefix}`
/// 验证 token_prefix 与服务端配置中 token 的前 8 位匹配
async fn validate_token(state: &AppState, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    // Token 格式: username-timestamp-token_prefix
    let parts: Vec<&str> = token.rsplitn(2, '-').collect();
    if parts.len() != 2 {
        return false;
    }

    let token_prefix = parts[0];
    let config = state.server_config().await;
    let expected = &config.server.token[..8.min(config.server.token.len())];

    token_prefix == expected
}
