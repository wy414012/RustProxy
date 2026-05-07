# RustProxy

轻量级内网穿透工具，使用 Rust 构建。

## 项目简介

RustProxy 是一个轻量、高性能的内网穿透工具，类似于 frp，但更简洁易用。它由服务端（Server）和客户端（Client）两部分组成，通过加密隧道建立安全连接，将内网服务暴露到公网。

### 核心特性

- **多协议支持** — TCP / UDP / HTTP / HTTPS 全协议穿透
- **Web 管理面板** — 服务端内置 Web UI，可视化管理端口映射
- **加密隧道** — 服务端与客户端之间基于 TLS + Token 的双向认证，自动生成自签证书
- **轻量高效** — 基于 Rust + Tokio 异步运行时，极低资源占用，单二文件部署
- **配置简单** — TOML 配置文件，开箱即用
- **热重载** — 支持运行时动态增删代理规则，无需重启

## 架构概览

```
┌─────────────────────────────────────────────────────┐
│                    RustProxy Server                  │
│  ┌──────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │ Web UI   │  │ Proxy Manager│  │  TLS Tunnel    │  │
│  │ (管理面板)│  │  (端口映射)   │  │  (加密隧道)    │  │
│  └──────────┘  └──────────────┘  └───────┬───────┘  │
│         │             │                    │         │
│  ┌──────┴─────────────┴────────────────────┘       │
│  │              Core Runtime (Tokio)               │  │
│  └──────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
                          │
                    TLS + Token 隧道
                          │
┌─────────────────────────────────────────────────────┐
│                    RustProxy Client                  │
│  ┌──────────────┐  ┌──────────────┐                  │
│  │  Proxy Worker│  │  TLS Tunnel  │                  │
│  │  (本地代理)   │  │  (加密隧道)   │                  │
│  └──────────────┘  └──────────────┘                  │
│  ┌──────────────────────────────────────────────────┐│
│  │              Core Runtime (Tokio)                 ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘
```

### 数据流

```
外部用户 ──▶ 服务端公网端口 ──▶ Proxy Manager ──▶ TLS 隧道 ──▶ 客户端 ──▶ 本地服务
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
| `rustproxy-core` | 共享基础设施：配置解析、日志、错误处理、工具函数 |
| `rustproxy-proto` | 通信协议：消息定义、序列化/反序列化、帧编码 |
| `rustproxy-server` | 服务端：隧道监听、代理管理、Web API、证书管理 |
| `rustproxy-client` | 客户端：隧道连接、本地代理转发、心跳保活 |
| `rustproxy-web` | Web 管理面板：嵌入式前端资源，编译进服务端二进制 |

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

### 服务端

```bash
# 生成默认配置
./rustproxy-server init

# 启动服务端
./rustproxy-server -c configs/server.toml
```

服务端启动后，默认 Web 管理面板监听 `http://0.0.0.0:7500`。

### 客户端

```bash
# 生成默认配置
./rustproxy-client init

# 启动客户端
./rustproxy-client -c configs/client.toml
```

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
bind_port = 7500
user = "admin"
password = "admin"

[tls]
auto_cert = true           # 自动生成自签证书
cert_file = ""             # 手动指定证书路径（留空则自动生成）
key_file = ""
```

**客户端配置** `configs/client.toml`：

```toml
[client]
server_addr = "your-server-ip"
server_port = 7000
token = "your-secret-token" # 需与服务端一致

[[proxy]]
name = "ssh"
type = "tcp"
local_ip = "127.0.0.1"
local_port = 22
remote_port = 6000          # 服务端暴露的公网端口

[[proxy]]
name = "web"
type = "http"
local_ip = "127.0.0.1"
local_port = 8080
custom_domains = ["web.example.com"]
```

## 代理类型

| 类型 | 说明 |
|------|------|
| `tcp` | TCP 端口映射，支持 SSH、数据库等任意 TCP 服务 |
| `udp` | UDP 端口映射，支持 DNS、游戏等 UDP 服务 |
| `http` | HTTP 代理，基于域名路由，支持多域名复用同一端口 |
| `https` | HTTPS 代理，服务端终止 TLS，后端连接本地 HTTP 服务 |

## 安全机制

- **TLS 加密隧道** — 服务端与客户端之间所有流量通过 TLS 加密传输
- **Token 鉴权** — 客户端连接时需携带与服务器一致的 Token，防止未授权接入
- **自签证书自动生成** — 首次启动自动生成服务端证书，也支持用户指定证书
- **Web 面板鉴权** — 管理面板支持用户名/密码认证

## 与 frp 的对比

| 特性 | RustProxy | frp |
|------|-----------|-----|
| 语言 | Rust | Go |
| 二进制大小 | ~5MB | ~15MB |
| 内存占用 | ~5MB | ~30MB |
| 依赖 | 零外部依赖 | 零外部依赖 |
| 配置复杂度 | 极简 | 较多选项 |
| Web 面板 | 内置 | 内置 |
| 插件系统 | 无 | 有 |
| 范围端口映射 | 规划中 | 有 |
| 加密压缩 | TLS | 可选 TLS + 压缩 |

## 开发路线

- [x] 项目架构设计
- [ ] 核心通信协议定义
- [ ] TLS 隧道建立与认证
- [ ] TCP 代理支持
- [ ] UDP 代理支持
- [ ] HTTP 代理支持
- [ ] HTTPS 代理支持
- [ ] Web 管理面板
- [ ] Web API（代理增删查改）
- [ ] 运行时热重载配置
- [ ] 流量统计
- [ ] Docker 镜像发布

## License

MIT
