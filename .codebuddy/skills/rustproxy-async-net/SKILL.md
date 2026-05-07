---
name: rustproxy-async-net
description: This skill should be used when developing RustProxy's async networking layer, including TLS tunnel establishment, Tokio-based connection management, codec implementation, TCP/UDP proxy forwarding, and multi-protocol proxy handling. It provides expert-level patterns for building production-grade async Rust networking applications with Tokio and tokio-rustls.
---

# RustProxy Async Networking Expert

## Purpose

Provide expert-level guidance for implementing RustProxy's async networking layer: TLS tunnels, connection lifecycle, frame codec, and multi-protocol (TCP/UDP/HTTP/HTTPS) proxy forwarding using Tokio + tokio-rustls.

## When to Use

- Implementing or modifying the TLS tunnel between server and client
- Building the frame codec (`FrameCodec`) for the control/data protocol
- Implementing TCP/UDP/HTTP/HTTPS proxy workers
- Managing async connection pools, heartbeat, and reconnection logic
- Handling graceful shutdown of network services
- Debugging async networking issues (hangs, leaks, connection drops)

## Architecture Reference

See `references/architecture.md` for the full networking architecture diagram and data flow.

## Core Patterns

### 1. TLS Tunnel Establishment

The server listens on `bind_port` with a TLS acceptor; the client connects with a TLS connector. Both use `tokio-rustls` (never native-tls).

```
Server                           Client
  │                                │
  │◄──── TCP Connect ──────────────│
  │◄──── TLS Handshake ───────────►│
  │◄──── AuthRequest (token) ──────│
  │──── AuthResponse ──────────────►│
  │◄──── RegisterProxy ────────────│
  │──── RegisterProxyResp ─────────►│
  │          (tunnel ready)         │
  │◄──── Ping ─────────────────────│
  │──── Pong ──────────────────────►│
```

**Server-side TLS setup:**
```rust,ignore
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

// Auto-generate self-signed cert with rcgen if not provided
fn build_server_tls_config(cert_path: &str, key_path: &str) -> Result<ServerConfig> {
    let (cert, key) = if cert_path.is_empty() {
        generate_self_signed_cert()?
    } else {
        (load_cert(cert_path)?, load_key(key_path)?)
    };
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    Ok(config)
}
```

**Client-side TLS setup:**
```rust,ignore
use rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use rustls::crypto::CryptoProvider;

// Accept self-signed certs (for auto-generated mode)
fn build_client_tls_config() -> Result<ClientConfig> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth();
    Ok(config)
}
```

**Critical:** `NoVerifier` must only be used when `auto_cert = true`. If user provides custom certs, use standard verification.

### 2. Frame Codec Usage

Use `tokio_util::codec::Framed` with `FrameCodec` for all control communication:

```rust,ignore
use tokio_util::codec::Framed;
use rustproxy_proto::frame::FrameCodec;
use rustproxy_proto::Message;

let framed = Framed::new(tls_stream, FrameCodec);

// Send control message
framed.send(Message::Control(ControlMessage::Auth(AuthRequest {
    token: config.client.token.clone(),
    version: VERSION.to_string(),
}))).await?;

// Receive message
while let Some(msg) = framed.next().await {
    match msg? {
        Message::Control(ctrl) => handle_control(ctrl).await,
        Message::Data(data) => handle_data(data).await,
    }
}
```

### 3. TCP Proxy Forwarding

For each TCP proxy rule, the server opens a public listener port. When an external user connects:

```
External User ──▶ Server Public Port ──▶ Find Client Connection
                                           │
                  Server tells client to open a "work connection"
                                           │
                  Client connects new TLS stream as work conn
                                           │
                  Server bridges: User TCP ◄──▶ Work Conn TLS ◄──▶ Client ◄──▶ Local Service
```

**Key implementation pattern — bidirectional copy:**
```rust,ignore
use tokio::io::{self, AsyncWriteExt};
use tokio::net::TcpStream;

async fn bidirectional_copy<A, B>(mut a: A, mut b: B) -> Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut a_read, mut a_write) = io::split(a);
    let (mut b_read, mut b_write) = io::split(b);

    let client_to_server = io::copy(&mut a_read, &mut b_write);
    let server_to_client = io::copy(&mut b_read, &mut a_write);

    tokio::select! {
        r = client_to_server => r?,
        r = server_to_client => r?,
    };
    // Handle results...
}
```

### 4. UDP Proxy Forwarding

UDP is connectionless. Use a mapping table with timeout:

```rust,ignore
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};

struct UdpSession {
    client_addr: SocketAddr,
    last_activity: Instant,
}

// Server-side: bind UdpSocket on remote_port
// Maintain HashMap<SocketAddr, UdpSession> for routing
// Expire sessions after 30s of inactivity
```

### 5. HTTP/HTTPS Proxy with Virtual Host Routing

HTTP proxies use `Host` header for routing; HTTPS uses SNI from TLS ClientHello:

```rust,ignore
// HTTP: parse the Host header from the first request bytes
// HTTPS: read SNI from TLS ClientHello before terminating TLS

fn extract_host_from_http_header(data: &[u8]) -> Option<String> {
    // Parse first line: "GET /path HTTP/1.1\r\nHost: example.com\r\n"
    let header = std::str::from_utf8(data).ok()?;
    for line in header.lines() {
        if let Some(host) = line.strip_prefix("Host: ") {
            return Some(host.trim().to_string());
        }
    }
    None
}

fn extract_sni_from_client_hello(data: &[u8]) -> Option<String> {
    // Parse TLS ClientHello to extract SNI extension
    // Reference: RFC 5246 Section 7.4.1.2
    // ...
}
```

### 6. Heartbeat & Reconnection

**Server-side:** Track last heartbeat per client, disconnect if timeout.

**Client-side:** Send Ping every 10s, reconnect on failure with exponential backoff:

```rust,ignore
use tokio::time::{sleep, Duration};
use std::time::Duration;

async fn run_with_reconnect<F, Fut>(mut connector: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(60);

    loop {
        match connector().await {
            Ok(()) => {
                delay = Duration::from_secs(1); // reset on success
            }
            Err(e) => {
                tracing::warn!("Connection lost: {}, reconnecting in {:?}", e, delay);
            }
        }
        sleep(delay).await;
        delay = (delay * 2).min(max_delay);
    }
}
```

### 7. Graceful Shutdown

Use `tokio::sync::broadcast` or `CancellationToken` for coordinated shutdown:

```rust,ignore
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();

// In each task: check token.is_cancelled() or select on token.cancelled()
tokio::select! {
    _ = do_work() => {},
    _ = token.cancelled() => {
        tracing::info!("Shutting down gracefully");
    }
}
```

## Common Pitfalls

1. **Never block the Tokio runtime** — Use `tokio::task::spawn_blocking` for CPU-heavy or blocking operations
2. **Always set timeouts** — Use `tokio::time::timeout` for all network operations; never assume a connection will respond
3. **Handle `UnexpectedEof` gracefully** — TLS connections can drop at any time; always match on `Err` variants
4. **Don't mix `std::net` and `tokio::net`** — Always use `tokio::net` types in async contexts
5. **`Framed` stream ends with `None`** — A `None` from `StreamExt::next()` means the connection closed; handle it explicitly
6. **TLS certificate rotation** — If certs change, existing connections keep the old cert; only new connections use the new cert
7. **UDP buffer size** — Set `UdpSocket::recv_buffer_size` appropriately; default OS buffer may be too small for high-throughput scenarios

## Dependency Quick Reference

| Crate | Version | Usage |
|-------|---------|-------|
| `tokio` | 1 | Async runtime, `features = ["full"]` |
| `tokio-util` | 0.7 | `codec` feature for `Framed` |
| `tokio-rustls` | 0.26 | TLS over Tokio streams |
| `rustls` | 0.23 | TLS configuration types |
| `rcgen` | 0.13 | Self-signed certificate generation |
| `bytes` | 1 | Zero-copy byte buffer (`Bytes`/`BytesMut`) |
