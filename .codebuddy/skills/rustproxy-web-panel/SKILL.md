---
name: rustproxy-web-panel
description: This skill should be used when developing the RustProxy web management panel, including Axum REST API design, embedded frontend assets, authentication middleware, real-time proxy status via WebSocket, and UI implementation with HTML/CSS/JS. It provides expert patterns for building lightweight embedded web panels in Rust.
---

# RustProxy Web Management Panel Expert

## Purpose

Provide expert-level guidance for implementing RustProxy's embedded web management panel: Axum REST API, authentication middleware, WebSocket real-time status, and a lightweight HTML/CSS/JS frontend compiled into the server binary via `include_str!` / `rust-embed`.

## When to Use

- Implementing or modifying Web API endpoints (proxy CRUD, status, stats)
- Building authentication middleware for the web panel
- Adding WebSocket support for real-time proxy status updates
- Implementing the frontend UI (HTML/CSS/JS)
- Embedding static assets into the server binary
- Designing the API response format and error handling

## Architecture Reference

See `references/api-design.md` for the full API specification.

## Core Patterns

### 1. Axum Application Setup

The web panel is built in `crates/web/` as a library, then used by `crates/server/`:

```rust,ignore
// crates/web/src/lib.rs
use axum::{routing::get, Router};
use crate::api::proxy_routes;
use crate::state::AppState;

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .nest("/api", proxy_routes())
        .route("/health", get(health_check))
        .fallback(serve_index)  // SPA fallback
        .layer(CorsLayer::permissive())
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state)
}
```

### 2. Shared State Design

Use `Arc` for thread-safe shared state. The state should hold a handle to the proxy manager:

```rust,ignore
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AppState {
    inner: Arc<AppStateInner>,
}

pub struct AppStateInner {
    pub proxy_manager: RwLock<ProxyManager>,
    pub stats: StatsCollector,
    pub web_config: WebConfig,
}
```

**Rules:**
- Use `tokio::sync::RwLock` (not `std::sync::RwLock`) — the async version doesn't hold locks across `.await` points
- For fast, short-lived reads, prefer `RwLock` with read guards
- For complex mutations, use `RwLock::write()` and keep the lock scope minimal
- Clone `AppState` freely — it's just an `Arc` clone

### 3. Authentication Middleware

Protect `/api/*` routes with a simple token-based auth:

```rust,ignore
use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth for /health and /api/auth/login
    let path = req.uri().path();
    if path == "/health" || path == "/api/auth/login" {
        return Ok(next.run(req).await);
    }

    // Check Authorization header or cookie
    let auth_header = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(token) if token == format!("Bearer {}", state.web_config.token) => {
            Ok(next.run(req).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
```

### 4. Login Endpoint

```rust,ignore
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    if req.username == state.web_config.user
        && req.password == state.web_config.password
    {
        // Generate a simple session token (or use JWT for production)
        let token = format!("{}-{}", req.username, uuid::Uuid::new_v4());
        Ok(Json(LoginResponse { token }))
    } else {
        Err(ApiError::Unauthorized("用户名或密码错误".into()))
    }
}
```

### 5. Unified API Response & Error Handling

Define a consistent response format:

```rust,ignore
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: u16,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self { code: 200, message: "ok".into(), data: Some(data) }
    }
}

pub enum ApiError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({
            "code": status.as_u16(),
            "message": message,
            "data": null,
        });
        (status, Json(body)).into_response()
    }
}
```

### 6. WebSocket Real-Time Updates

Push proxy status changes to the frontend via WebSocket:

```rust,ignore
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
};

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        interval.tick().await;

        let proxies = state.inner.proxy_manager.read().await.list_all();
        let msg = serde_json::to_string(&proxies).unwrap();

        if socket.send(Message::Text(msg.into())).await.is_err() {
            break; // client disconnected
        }
    }
}
```

### 7. Embedding Static Assets

Use `rust-embed` to compile frontend files into the binary:

**Dependency** (add to `crates/web/Cargo.toml`):
```toml
rust-embed = { version = "8", features = ["mime-guess"] }
```

**Usage:**
```rust,ignore
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

async fn serve_static(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = file.metadata.mimetype()?;
    Some((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime)],
        file.data.to_vec(),
    ).into_response())
}

async fn serve_index() -> impl IntoResponse {
    serve_static("index.html").unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
```

**Alternative without `rust-embed`:** Use `include_str!` for small files:
```rust,ignore
const INDEX_HTML: &str = include_str!("../../assets/index.html");
```

### 8. Frontend Architecture

Keep the frontend lightweight — no build tools, no frameworks. Pure HTML + CSS + vanilla JS:

```
assets/
├── index.html          # Single-page app shell
├── css/
│   └── style.css       # All styles
└── js/
    ├── app.js          # Main entry, router
    ├── api.js          # API client wrapper
    └── proxy.js        # Proxy CRUD UI logic
```

**Key frontend patterns:**

```javascript
// api.js — centralized API client
const API_BASE = '/api';

async function request(method, path, body) {
    const token = localStorage.getItem('token');
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers['Authorization'] = `Bearer ${token}`;

    const res = await fetch(`${API_BASE}${path}`, {
        method,
        headers,
        body: body ? JSON.stringify(body) : undefined,
    });

    const data = await res.json();
    if (data.code !== 200) throw new Error(data.message);
    return data.data;
}

// proxy.js — Proxy management
async function loadProxies() {
    const proxies = await request('GET', '/proxies');
    renderProxyTable(proxies);
}

// WebSocket for real-time updates
function connectWS() {
    const ws = new WebSocket(`ws://${location.host}/api/ws`);
    ws.onmessage = (e) => {
        const data = JSON.parse(e.data);
        updateProxyStatus(data);
    };
    ws.onclose = () => setTimeout(connectWS, 3000); // auto reconnect
}
```

### 9. API Route Organization

Keep routes in `api.rs` organized by resource:

```rust,ignore
pub fn proxy_routes() -> Router<AppState> {
    Router::new()
        // Auth
        .route("/auth/login", post(login))
        // Proxy CRUD
        .route("/proxies", get(list_proxies).post(create_proxy))
        .route("/proxies/:name", get(get_proxy).delete(delete_proxy).put(update_proxy))
        // Status & Stats
        .route("/status", get(server_status))
        .route("/stats/:name", get(proxy_stats))
        // WebSocket
        .route("/ws", get(ws_handler))
}
```

## Common Pitfalls

1. **Don't use `std::sync::RwLock` in async code** — It can deadlock across `.await` points. Always use `tokio::sync::RwLock`
2. **Don't hold locks across `.await`** — Keep lock scopes minimal; clone data out of the lock before awaiting
3. **Don't use `todo!()` in production handlers** — Replace with proper error responses before merging
4. **Always validate user input** — Never trust client-side data; validate proxy names, ports, IPs server-side
5. **Rate limit the login endpoint** — Prevent brute force on the admin panel
6. **Use `#[allow(dead_code)]` sparingly** — Only as temporary scaffolding with a TODO comment; remove before merging
7. **Frontend: avoid npm** — The web panel should be pure HTML/CSS/JS, no build step, no node_modules
8. **WebSocket reconnection** — The client must auto-reconnect on disconnect with backoff
9. **SPA routing** — All unknown paths should fallback to `index.html` for the SPA router

## Dependency Quick Reference

| Crate | Version | Usage |
|-------|---------|-------|
| `axum` | 0.7 | Web framework |
| `tower-http` | 0.5 | CORS, static file serving, trace |
| `serde` / `serde_json` | 1 | Request/response serialization |
| `tokio` | 1 | Async runtime |
| `rust-embed` | 8 | Compile-time static asset embedding |
