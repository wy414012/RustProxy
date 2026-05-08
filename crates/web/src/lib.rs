//! RustProxy Web — 嵌入式 Web 管理面板
//!
//! 提供代理规则的增删查改、客户端状态监控、服务端状态查询等管理功能。
//! 前端资源通过 `rust-embed` 编译时嵌入二进制，无需外部文件。

pub mod api;
pub mod auth;
pub mod state;
pub mod ws;

use axum::{
    http::{header, StatusCode},
    middleware,
    response::IntoResponse,
    routing::get,
    Router,
};
use rust_embed::Embed;
use tower_http::cors::CorsLayer;

use crate::api::proxy_routes;
use crate::state::AppState;

/// 嵌入的前端静态资源
#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

/// 构建 Web 应用路由
///
/// `cors_origins` 为 CORS 允许的域名列表，留空则仅允许同源访问（推荐生产环境留空）。
pub fn build_app(state: AppState, cors_origins: Vec<String>) -> Router {
    // API 路由 + 认证中间件
    let api_routes =
        proxy_routes()
            .route("/ws", get(ws::ws_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::auth_middleware,
            ));

    let mut router = Router::new()
        .nest("/api", api_routes)
        .route("/health", get(health_check))
        .fallback(static_handler)
        .with_state(state);

    // 仅在配置了 CORS 源时添加 CORS 层，否则仅允许同源访问
    if !cors_origins.is_empty() {
        let cors = build_cors_layer(&cors_origins);
        router = router.layer(cors);
    }

    router
}

/// 根据配置构建 CORS 层
fn build_cors_layer(origins: &[String]) -> CorsLayer {
    use axum::http::Method;

    let allowed_origins: Vec<_> = origins
        .iter()
        .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(allowed_origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}

async fn health_check() -> &'static str {
    "OK"
}

/// 静态资源处理（SPA 回退）
///
/// 仅对非 API 路径生效。API 路径如果未匹配路由，返回 404 JSON。
async fn static_handler(uri: axum::http::Uri) -> impl axum::response::IntoResponse {
    let path = uri.path();

    // API 路径未匹配到任何路由，返回 404 JSON
    if path.starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"code":404,"message":"接口不存在","data":null}"#,
        )
            .into_response();
    }

    let clean_path = path.trim_start_matches('/');

    // 尝试精确匹配
    if let Some(file) = Assets::get(clean_path) {
        return serve_file(&file, clean_path);
    }

    // SPA 回退：返回 index.html
    match Assets::get("index.html") {
        Some(file) => serve_file(&file, "index.html"),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// 根据文件扩展名返回正确的 Content-Type
fn serve_file(file: &rust_embed::EmbeddedFile, path: &str) -> axum::response::Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.as_ref())],
        file.data.to_vec(),
    )
        .into_response()
}
