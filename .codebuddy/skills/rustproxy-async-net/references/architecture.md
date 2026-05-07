# RustProxy Networking Architecture

## Overall Architecture

```
┌───────────────────────────────────────────────────────────────┐
│                        RustProxy Server                        │
│                                                               │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────────┐ │
│  │ Tunnel       │  │ ProxyManager │  │ Public Listeners      │ │
│  │ Listener    │  │              │  │ (TCP/UDP/HTTP/HTTPS)  │ │
│  │ (TLS:7000)  │  │ - proxy_map  │  │                       │ │
│  │             │  │ - port_map    │  │ TCP  :6000,6001,...   │ │
│  └──────┬──────┘  └──────┬───────┘  │ UDP  :7000,7001,...   │ │
│         │                │          │ HTTP :80 (vhost)      │ │
│  ┌──────┴────────────────┴──────────┴─────────────────────┐  │
│  │              ClientSession Manager                       │  │
│  │  - tracks connected clients                            │  │
│  │  - routes work connections to public listeners          │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Core Runtime (Tokio)                        │  │
│  └─────────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────┘
                              │
                        TLS Tunnel
                        (port 7000)
                              │
┌───────────────────────────────────────────────────────────────┐
│                        RustProxy Client                        │
│                                                               │
│  ┌─────────────┐  ┌──────────────────────────────────────┐    │
│  │ Tunnel      │  │ Proxy Workers                       │    │
│  │ Connector   │  │ - TCP: connect local, relay data     │    │
│  │ (TLS)       │  │ - UDP: bind local, relay packets    │    │
│  │             │  │ - HTTP: connect local HTTP           │    │
│  └──────┬──────┘  │ - HTTPS: connect local HTTPS         │    │
│         │         └──────────────────────────────────────┘    │
│  ┌──────┴──────────────────────────────────────────────────┐  │
│  │  Work Connection Pool                                    │  │
│  │  - pre-established TLS connections ready for data       │  │
│  │  - one work conn per active proxy stream               │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Core Runtime (Tokio)                        │  │
│  └─────────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────┘
```

## Connection Types

### Control Connection (1 per client)
- Persistent TLS connection on `server.bind_port`
- Used for: Auth, RegisterProxy, Heartbeat, CloseProxy
- Messages: `ControlMessage` enum (JSON-serialized)

### Work Connection (N per client, 1 per active stream)
- Additional TLS connections opened on demand
- Triggered by `NewWorkConn` control message
- Used for: Data relay (bidirectional copy)
- Messages: `DataMessage` (binary-encoded: `u64 conn_id + raw bytes`)

### Public Listener (managed by server)
- TCP: One `TcpListener` per `remote_port`
- UDP: One `UdpSocket` per `remote_port`
- HTTP: Single `TcpListener` on port 80, route by `Host` header
- HTTPS: Single `TcpListener` on port 443, route by SNI

## Data Flow — TCP Proxy Example

```
1. External user connects to server:6000
2. Server accepts connection, looks up proxy "ssh" on port 6000
3. Server sends NewWorkConn("ssh") to client via control connection
4. Client opens a new TLS work connection to server
5. Server bridges: user TCP socket ◄──► work connection TLS stream
6. Client bridges: work connection TLS stream ◄──► local 127.0.0.1:22
7. Bidirectional copy runs until either side closes
8. Server and client clean up the work connection
```

## Data Flow — HTTP Proxy Example

```
1. External user sends HTTP request to server:80 with Host: web.example.com
2. Server reads the Host header, looks up proxy "web" with domain "web.example.com"
3. Server sends NewWorkConn("web") to client via control connection
4. Client opens a new TLS work connection to server
5. Server bridges: user TCP socket ◄──► work connection TLS stream
6. Client bridges: work connection TLS stream ◄──► local 127.0.0.1:8080
7. Original HTTP request bytes are forwarded through the work connection
```
