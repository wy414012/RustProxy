# RustProxy

轻量级内网穿透工具，使用 Rust 构建。

## 项目简介

RustProxy 是一个轻量、高性能的内网穿透工具，类似于 frp，但更简洁易用。它由服务端（Server）和客户端（Client）两部分组成，通过加密隧道建立安全连接，将内网服务暴露到公网。

### 核心特性

- **多协议支持** — TCP / UDP / HTTP / HTTPS 全协议穿透
- **Web 管理面板** — 服务端内置 Web UI，可视化管理端口映射，代理规则集中管理
- **加密隧道** — 服务端与客户端之间基于 TLS + Token 的双向认证，自动生成自签证书
- **轻量高效** — 基于 Rust + Tokio 异步运行时，极低资源占用，单二文件部署
- **配置极简** — 客户端仅配置服务端地址和 Token，代理规则由 Web 面板统一管理
- **动态管理** — 运行时通过 Web 面板增删改代理规则，实时生效，无需重启

## 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                       RustProxy Server                       │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ Web UI   │  │ Proxy Listener│  │    TLS Tunnel        │   │
│  │ (管理面板)│  │ (公网端口监听) │  │   (加密隧道+认证)     │   │
│  └─────┬────┘  └──────┬───────┘  └──────────┬───────────┘   │
│        │              │                      │               │
│  ┌─────┴──────────────┴──────────────────────┘               │
│  │              Core Runtime (Tokio) + SQLite                │
│  └───────────────────────────────────────────────────────────┘
└─────────────────────────────────────────────────────────────┘
                              │
                     TLS + Token 隧道
                              │
┌─────────────────────────────────────────────────────────────┐
│                       RustProxy Client                       │
│  ┌──────────────┐  ┌──────────────┐                         │
│  │  Proxy Worker│  │  TLS Tunnel  │   客户端无需配置代理规则  │
│  │  (本地代理)   │  │  (加密隧道)   │   认证后自动接收服务端   │
│  └──────┬───────┘  └──────┬───────┘   下发的代理规则        │
│         │                 │                                  │
│  ┌──────┴─────────────────┘                                  │
│  │              Core Runtime (Tokio)                         │
│  └───────────────────────────────────────────────────────────┘
└─────────────────────────────────────────────────────────────┘
```

### 工作流程

```
1. 客户端连接服务端，发送 Auth（携带 client_id + token）
2. 服务端认证成功后，推送该 client_id 的所有代理规则（ServerAssignProxy）
3. 服务端为 TCP/UDP 代理规则创建公网端口监听器
4. 外部用户访问服务端公网端口 → 服务端通知客户端建立工作连接（NewWorkConn）
5. 客户端连接本地服务，建立双向数据转发

数据流:
外部用户 ──▶ 服务端公网端口 ──▶ Proxy Listener ──▶ TLS 隧道 ──▶ 客户端 ──▶ 本地服务
```

## 项目结构

```
rustproxy/
├── Cargo.toml                  # Workspace 根配置
├── README.md
├── LICENSE
├── configs/
│   ├── server.toml             # 服务端配置模板
│   └── client.toml             # 客户端配置模板
└── crates/
    ├── core/                   # 核心共享库
    │   └── Cargo.toml
    ├── proto/                  # 通信协议定义
    │   └── Cargo.toml
    ├── server/                 # 服务端
    │   └── Cargo.toml
    ├── client/                 # 客户端
    │   └── Cargo.toml
    └── web/                    # Web 管理面板前端
        └── Cargo.toml
```

### Crate 职责

| Crate | 说明 |
|-------|------|
| `rustproxy-core` | 共享基础设施：配置解析、日志、错误处理、代理规则管理、SQLite 持久化 |
| `rustproxy-proto` | 通信协议：消息定义、序列化/反序列化、帧编解码 |
| `rustproxy-server` | 服务端：隧道监听、公网端口监听、工作连接桥接、Web API、证书管理 |
| `rustproxy-client` | 客户端：隧道连接、本地代理转发、工作连接管理、心跳保活 |
| `rustproxy-web` | Web 管理面板：REST API、嵌入式前端资源，编译进服务端二进制 |

## 快速开始

### 安装

```bash
# 从源码编译（需要 Rust 工具链）
git clone https://github.com/yourname/rustproxy.git
cd rustproxy
cargo build --release

# 编译产物位于 target/release/
# - rustproxy-server
# - rustproxy-client
```

### 第一步：启动服务端

```bash
# 编辑配置文件
vim configs/server.toml

# 启动服务端
./rustproxy-server -c configs/server.toml
```

服务端启动后：
- **隧道端口** 默认监听 `0.0.0.0:7000`（客户端连接用）
- **Web 面板** 默认监听 `http://0.0.0.0:7500`
- **自签证书** 首次启动自动生成并保存（`auto_cert = true` 时）

### 第二步：通过 Web 面板添加代理规则

打开浏览器访问 `http://your-server-ip:7500`，登录后添加代理规则：

| 字段 | 说明 | 示例 |
|------|------|------|
| 名称 | 代理规则唯一标识 | `ssh` |
| 类型 | 代理协议 | `tcp` |
| 客户端 ID | 关联的客户端标识 | `my-server` |
| 本地地址 | 客户端本地服务 IP | `127.0.0.1` |
| 本地端口 | 客户端本地服务端口 | `22` |
| 远程端口 | 服务端暴露的公网端口（TCP/UDP） | `6000` |

> **重要**：代理规则由服务端集中管理，客户端无需任何代理配置。服务端创建规则后自动开启公网端口监听。

### 第三步：启动客户端

```bash
# 编辑配置文件（仅需服务端连接信息）
vim configs/client.toml

# 启动客户端
./rustproxy-client -c configs/client.toml
```

客户端连接成功后，自动接收服务端下发的代理规则，开始转发流量。

### 配置示例

**服务端配置** `configs/server.toml`：

```toml
[server]
bind_addr = "0.0.0.0"
bind_port = 7000           # 隧道监听端口
token = "your-secret-token" # 鉴权 Token

[web]
enable = true
bind_addr = "0.0.0.0"
bind_port = 7500           # Web 管理面板端口
user = "admin"
password = "admin123"

[tls]
auto_cert = true           # 自动生成自签证书
cert_file = "certs/server.crt"
key_file = "certs/server.key"
```

**客户端配置** `configs/client.toml`（极简，仅需连接信息）：

```toml
[client]
id = "my-server"                   # 客户端唯一标识（Web 面板创建规则时关联此 ID）
server_addr = "your-server-ip"     # 服务端地址
server_port = 7000                 # 服务端隧道端口
token = "your-secret-token"        # 需与服务端一致
# ca_cert = ""                     # 服务端 CA 证书路径，留空则信任自签证书
```

> **注意**：客户端不需要配置任何代理规则！所有代理规则通过 Web 面板管理，客户端认证后自动接收。

## 代理类型

| 类型 | 说明 | 配置要点 |
|------|------|----------|
| `tcp` | TCP 端口映射，支持 SSH、数据库等任意 TCP 服务 | 需指定 `remote_port`（服务端公网端口） |
| `udp` | UDP 端口映射，支持 DNS、游戏等 UDP 服务 | 需指定 `remote_port` |
| `http` | HTTP 代理，基于域名路由，多域名复用同一端口 | 需指定 `custom_domains` |
| `https` | HTTPS 代理，服务端终止 TLS，后端连接本地 HTTP 服务 | 需指定 `custom_domains` |

## 安全机制

- **TLS 加密隧道** — 服务端与客户端之间所有流量通过 TLS 加密传输
- **Token 鉴权** — 客户端连接时需携带与服务器一致的 Token，防止未授权接入
- **自签证书自动生成** — 首次启动自动生成服务端证书，也支持用户指定证书
- **Web 面板鉴权** — 管理面板支持用户名/密码认证，API 需携带 JWT Token

## Web API

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/auth/login` | 登录获取 Token |
| GET | `/api/proxies` | 获取所有代理规则 |
| GET | `/api/proxies/:name` | 获取单个代理规则 |
| POST | `/api/proxies` | 创建代理规则 |
| PUT | `/api/proxies/:name` | 更新代理规则 |
| DELETE | `/api/proxies/:name` | 删除代理规则 |
| GET | `/api/clients` | 获取在线客户端列表 |

> 创建/更新/删除代理规则后，服务端自动通知关联客户端，实时生效。

## 与 frp 的对比

| 特性 | RustProxy | frp |
|------|-----------|-----|
| 语言 | Rust | Go |
| 二进制大小 | ~5MB | ~15MB |
| 内存占用 | ~5MB | ~30MB |
| 代理规则管理 | 服务端 Web 面板集中管理 | 客户端配置文件 |
| 配置复杂度 | 客户端极简（仅连接信息） | 较多选项 |
| Web 面板 | 内置 | 内置 |
| 加密通信 | TLS（默认开启） | 可选 TLS |
| 范围端口映射 | 规划中 | 有 |
| 流量统计 | 规划中 | 有 |

## 开发路线

- [x] 项目架构设计
- [x] 核心通信协议定义（帧编解码 + 控制消息 + 数据消息）
- [x] TLS 隧道建立与 Token 认证
- [x] 自签证书自动生成
- [x] Web 管理面板（代理规则 CRUD + 在线客户端查看）
- [x] Web API（REST + JWT 鉴权）
- [x] TCP 代理支持（公网监听 + 工作连接 + 双向数据转发）
- [x] UDP 代理支持（工作连接隧道传输 UDP 数据包）
- [x] HTTP 代理支持（共享端口，基于 Host 头域名路由）
- [x] HTTPS 代理支持（共享端口，SNI 路由 + TLS 终止）
- [x] 流量统计（实时字节级入站/出站统计）
- [x] 运行时动态管理代理规则（创建/更新/删除实时生效）

## License

MIT
