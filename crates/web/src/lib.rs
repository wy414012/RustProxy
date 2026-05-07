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
pub fn build_app(state: AppState) -> Router {
    let api_routes =
        proxy_routes()
            .route("/ws", get(ws::ws_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::auth_middleware,
            ));

    Router::new()
        .nest("/api", api_routes)
        .route("/health", get(health_check))
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}

/// 静态资源处理（SPA 回退）
///
/// 尝试匹配嵌入的静态文件，找不到则返回 index.html（SPA 路由）。
async fn static_handler(uri: axum::http::Uri) -> impl axum::response::IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // 尝试精确匹配
    if let Some(file) = Assets::get(path) {
        return serve_file(&file, path);
    }

    // SPA 回退：所有非 API、非静态文件路径返回 index.html
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
