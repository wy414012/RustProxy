//! 认证中间件
//!
//! 保护 `/api/*` 路由（除登录接口外），验证 JWT Token。
//! JWT 使用 HS256 算法签名，密钥为服务端配置中的 `server.token`。

use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};

use crate::state::AppState;

/// JWT Claims 结构
#[derive(serde::Serialize, serde::Deserialize)]
pub struct JwtClaims {
    /// 用户名（subject）
    pub sub: String,
    /// 过期时间（Unix 时间戳，秒）
    pub exp: usize,
    /// 签发时间（Unix 时间戳，秒）
    pub iat: usize,
}

/// 生成 JWT Token
pub fn generate_jwt(username: &str, secret: &str, expire_hours: u64) -> Result<String, String> {
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("时间获取失败: {}", e))?;

    let claims = JwtClaims {
        sub: username.to_string(),
        iat: now.as_secs() as usize,
        exp: (now.as_secs() + expire_hours * 3600) as usize,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("Token 生成失败: {}", e))
}

/// 验证 JWT Token，返回解析后的 Claims
pub fn validate_jwt(token: &str, secret: &str) -> Result<JwtClaims, String> {
    use jsonwebtoken::{decode, DecodingKey, Validation};

    decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| "Token 无效或已过期".to_string())
}

/// API 认证中间件
///
/// 跳过 `/health`、`/api/auth/login` 和 WebSocket 握手路径，
/// 其余请求需携带有效的 Bearer Token（JWT）。
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
            let config = state.server_config().await;
            let secret = config.server.token.clone();
            drop(config);

            if validate_jwt(token, &secret).is_ok() {
                Ok(next.run(req).await)
            } else {
                Err(axum::http::StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(axum::http::StatusCode::UNAUTHORIZED),
    }
}
