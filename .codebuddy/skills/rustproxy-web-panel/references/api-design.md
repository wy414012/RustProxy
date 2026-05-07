# RustProxy Web API Specification

## Base URL

```
/api
```

## Authentication

All endpoints except `/api/auth/login` require an `Authorization` header:
```
Authorization: Bearer <token>
```

Token is obtained via the login endpoint.

---

## Endpoints

### POST /api/auth/login

Authenticate and obtain a session token.

**Request:**
```json
{
  "username": "admin",
  "password": "admin"
}
```

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "token": "admin-550e8400-e29b-41d4-a716-446655440000"
  }
}
```

**Response (401):**
```json
{
  "code": 401,
  "message": "用户名或密码错误",
  "data": null
}
```

---

### GET /api/proxies

List all registered proxy rules with their status.

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": [
    {
      "name": "ssh",
      "proxy_type": "tcp",
      "local_addr": "127.0.0.1:22",
      "remote_port": 6000,
      "status": "running",
      "connections": 3,
      "bytes_in": 102400,
      "bytes_out": 204800
    },
    {
      "name": "web",
      "proxy_type": "http",
      "local_addr": "127.0.0.1:8080",
      "remote_port": 0,
      "custom_domains": ["web.example.com"],
      "status": "running",
      "connections": 12,
      "bytes_in": 5242880,
      "bytes_out": 10485760
    }
  ]
}
```

---

### POST /api/proxies

Create a new proxy rule dynamically (hot-reload).

**Request:**
```json
{
  "name": "mysql",
  "proxy_type": "tcp",
  "local_ip": "127.0.0.1",
  "local_port": 3306,
  "remote_port": 63306,
  "custom_domains": []
}
```

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "name": "mysql",
    "proxy_type": "tcp",
    "local_addr": "127.0.0.1:3306",
    "remote_port": 63306,
    "status": "starting",
    "connections": 0,
    "bytes_in": 0,
    "bytes_out": 0
  }
}
```

**Response (400):**
```json
{
  "code": 400,
  "message": "代理规则名称已存在: mysql",
  "data": null
}
```

---

### GET /api/proxies/:name

Get details of a specific proxy rule.

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "name": "ssh",
    "proxy_type": "tcp",
    "local_addr": "127.0.0.1:22",
    "remote_port": 6000,
    "status": "running",
    "connections": 3,
    "bytes_in": 102400,
    "bytes_out": 204800,
    "started_at": "2025-05-07T10:30:00Z"
  }
}
```

**Response (404):**
```json
{
  "code": 404,
  "message": "代理规则不存在: foo",
  "data": null
}
```

---

### PUT /api/proxies/:name

Update an existing proxy rule (requires restart of the proxy).

**Request:**
```json
{
  "local_port": 2222,
  "remote_port": 6001
}
```

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "name": "ssh",
    "proxy_type": "tcp",
    "local_addr": "127.0.0.1:2222",
    "remote_port": 6001,
    "status": "restarting",
    "connections": 0,
    "bytes_in": 0,
    "bytes_out": 0
  }
}
```

---

### DELETE /api/proxies/:name

Remove a proxy rule and stop its listener.

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": null
}
```

---

### GET /api/status

Get server status overview.

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "version": "0.1.0",
    "uptime_secs": 86400,
    "connected_clients": 2,
    "total_proxies": 5,
    "active_connections": 15,
    "total_bytes_in": 10485760,
    "total_bytes_out": 20971520
  }
}
```

---

### GET /api/stats/:name

Get traffic statistics for a specific proxy.

**Response (200):**
```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "name": "ssh",
    "total_connections": 42,
    "current_connections": 3,
    "total_bytes_in": 1024000,
    "total_bytes_out": 2048000,
    "last_connection_at": "2025-05-07T15:42:00Z"
  }
}
```

---

### GET /api/ws

WebSocket endpoint for real-time proxy status updates.

**Messages (server → client, every 2 seconds):**
```json
[
  {
    "name": "ssh",
    "status": "running",
    "connections": 3,
    "bytes_in": 102400,
    "bytes_out": 204800
  },
  {
    "name": "web",
    "status": "running",
    "connections": 12,
    "bytes_in": 5242880,
    "bytes_out": 10485760
  }
]
```

---

## Error Response Format

All errors follow the same structure:

```json
{
  "code": <HTTP status code>,
  "message": "<human-readable error description>",
  "data": null
}
```

## Proxy Status Values

| Status | Description |
|--------|-------------|
| `starting` | Proxy rule registered, listener opening |
| `running` | Proxy active and accepting connections |
| `restarting` | Proxy being reconfigured |
| `stopping` | Proxy shutting down |
| `stopped` | Proxy inactive |
| `error` | Proxy failed (see message for details) |
