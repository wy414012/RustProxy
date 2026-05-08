# RustProxy 部署指南

本指南帮助你从零开始部署 RustProxy 服务端和客户端。

## 目录

- [系统要求](#系统要求)
- [下载](#下载)
- [服务端部署](#服务端部署)
- [客户端部署](#客户端部署)
- [配置说明](#配置说明)
- [TLS 证书](#tls-证书)
- [常见问题](#常见问题)

## 系统要求

- Linux 系统（x86_64 或 aarch64/arm64）
- 无需安装任何运行时依赖（musl 静态编译，零依赖）

## 下载

从 [Release 页面](https://cnb.cool/emchaye/RustProxy/-/releases) 下载对应架构的压缩包：

| 文件名 | 架构 | 包含内容 |
|--------|------|----------|
| `rustproxy-server-x86_64-musl.tar.gz` | x86_64 (amd64) | 二进制 + 配置模板 + systemd 服务 |
| `rustproxy-server-aarch64-musl.tar.gz` | aarch64 (arm64) | 同上 |
| `rustproxy-client-x86_64-musl.tar.gz` | x86_64 (amd64) | 二进制 + 配置模板 + systemd 服务 |
| `rustproxy-client-aarch64-musl.tar.gz` | aarch64 (arm64) | 同上 |

> 服务端部署在公网服务器上，客户端部署在内网机器上。两者不需要在同一台机器。

---

## 服务端部署

### 1. 创建目录并解压

```bash
mkdir -p /home/rustproxy/server
cd /home/rustproxy/server

# x86_64 服务器
tar xzf rustproxy-server-x86_64-musl.tar.gz

# 或 aarch64 服务器
# tar xzf rustproxy-server-aarch64-musl.tar.gz
```

解压后目录结构：

```
/home/rustproxy/server/
├── rustproxy-server          # 服务端二进制
├── server.toml               # 配置文件
└── rustproxy-server.service  # systemd 服务文件
```

### 2. 修改配置

```bash
vim server.toml
```

**必须修改的配置项：**

```toml
[server]
# ⚠️ 必须修改！客户端连接时需携带相同 Token
token = "你的随机密钥"

[web]
# ⚠️ 必须修改！Web 面板登录密码（支持明文或 bcrypt 哈希，推荐 bcrypt）
password = "你的强密码"
# JWT Token 过期时间（小时），默认 24
# token_expire_hours = 24
# 允许访问 Web 面板的外部网站域名（一般留空即可）
# 留空 = 只有通过面板本身地址访问才有效，其他网站无法冒用你的面板
# 如果你的面板地址是 http://1.2.3.4:7500，那直接浏览器访问就没问题
# 只有当你需要从其他网站（如 http://my-site.com）调用面板 API 时才需要填写
# cors_origins = []
```

> 其他配置项使用默认值即可，首次启动时会自动生成 TLS 自签证书并保存到 `certs/` 目录。

### 3. 启动服务

**方式一：手动启动（测试用）**

```bash
./rustproxy-server
```

服务启动后：
- 隧道端口：`0.0.0.0:7000`（客户端连接用）
- Web 面板：`http://0.0.0.0:7500`
- 自动生成自签证书保存到 `certs/` 目录

**方式二：systemd 守护进程（生产推荐）**

```bash
# 安装 systemd 服务文件
cp rustproxy-server.service /etc/systemd/system/

# 加载并启动
systemctl daemon-reload
systemctl enable --now rustproxy-server

# 查看状态
systemctl status rustproxy-server

# 查看日志
journalctl -u rustproxy-server -f
```

### 4. 开放防火墙端口

```bash
# 隧道端口（客户端连接）
firewall-cmd --permanent --add-port=7000/tcp

# Web 管理面板
firewall-cmd --permanent --add-port=7500/tcp

# 代理端口（根据实际使用的远程端口开放，如 6000-6100）
firewall-cmd --permanent --add-port=6000-6100/tcp

# HTTP/HTTPS 代理端口（如果启用）
firewall-cmd --permanent --add-port=8080/tcp
firewall-cmd --permanent --add-port=8443/tcp

firewall-cmd --reload
```

### 5. 通过 Web 面板添加代理规则

浏览器访问 `http://你的服务器IP:7500`，使用配置文件中的账号密码登录。

添加代理规则示例：

| 字段 | 值 | 说明 |
|------|----|------|
| 名称 | `ssh` | 规则唯一标识 |
| 类型 | `tcp` | TCP 端口映射 |
| 客户端 ID | `my-server` | 对应客户端配置中的 `id` |
| 本地地址 | `127.0.0.1` | 客户端本地的服务地址 |
| 本地端口 | `22` | 客户端本地的服务端口 |
| 远程端口 | `6000` | 服务端暴露的公网端口 |

创建规则后，当客户端连接时会自动接收该规则并开始转发。

---

## 客户端部署

### 1. 创建目录并解压

```bash
mkdir -p /home/rustproxy/client
cd /home/rustproxy/client

# x86_64 机器
tar xzf rustproxy-client-x86_64-musl.tar.gz

# 或 aarch64 机器
# tar xzf rustproxy-client-aarch64-musl.tar.gz
```

解压后目录结构：

```
/home/rustproxy/client/
├── rustproxy-client          # 客户端二进制
├── client.toml               # 配置文件
└── rustproxy-client.service  # systemd 服务文件
```

### 2. 修改配置

```bash
vim client.toml
```

**必须修改的配置项：**

```toml
[client]
# 客户端唯一标识，需与 Web 面板中代理规则的「客户端 ID」一致
id = "my-server"

# 服务端公网地址
server_addr = "你的服务器IP"
server_port = 7000

# 必须与服务端 token 一致
token = "你的随机密钥"

# ⚠️ 生产环境推荐：拷贝服务端证书到客户端并指定路径
# 将服务端 certs/server.crt 复制到客户端机器，例如 certs/ca.crt
# ca_cert = "certs/ca.crt"
```

> 客户端不需要配置任何代理规则！所有规则由服务端 Web 面板管理，客户端认证后自动接收。

### 3. 启动服务

**方式一：手动启动（测试用）**

```bash
./rustproxy-client
```

**方式二：systemd 守护进程（生产推荐）**

```bash
# 安装 systemd 服务文件
cp rustproxy-client.service /etc/systemd/system/

# 加载并启动
systemctl daemon-reload
systemctl enable --now rustproxy-client

# 查看状态
systemctl status rustproxy-client

# 查看日志
journalctl -u rustproxy-client -f
```

### 4. 验证连接

客户端启动后日志中应出现：

```
INFO rustproxy_client: 已连接到服务端
INFO rustproxy_client: 收到代理规则: ssh (tcp, 127.0.0.1:22 -> :6000)
```

此时即可通过 `服务器IP:6000` 访问内网的 SSH 服务。

---

## 配置说明

### 服务端配置 (`server.toml`)

```toml
[server]
bind_addr = "0.0.0.0"       # 隧道监听地址
bind_port = 7000             # 隧道监听端口
token = "CHANGE_ME"          # 客户端认证 Token（⚠️ 必须修改）
http_port = 8080             # HTTP 代理端口（0 = 不监听）
https_port = 8443            # HTTPS 代理端口（0 = 不监听）

[web]
enable = true                # 是否启用 Web 面板
bind_addr = "0.0.0.0"       # Web 面板监听地址
bind_port = 7500             # Web 面板端口
user = "admin"               # Web 面板用户名
password = "CHANGE_ME"       # Web 面板密码（⚠️ 必须修改，支持明文或 bcrypt 哈希）
token_expire_hours = 24      # JWT Token 过期时间（小时）
cors_origins = []            # 允许访问面板的外部网站（留空=只有直接访问面板地址才有效）

[tls]
auto_cert = true             # 自动生成自签证书
cert_file = "certs/server.crt"  # 证书路径
key_file = "certs/server.key"   # 密钥路径
```

### 客户端配置 (`client.toml`)

```toml
[client]
id = "my-laptop"             # 客户端唯一标识
server_addr = "127.0.0.1"    # 服务端地址
server_port = 7000           # 服务端隧道端口
token = "CHANGE_ME"          # 与服务端一致的 Token（⚠️ 必须修改）
ca_cert = ""                 # CA 证书路径（⚠️ 生产环境推荐配置，见下方说明）
```

### 代理类型说明

| 类型 | 说明 | 需要的字段 |
|------|------|------------|
| `tcp` | TCP 端口映射 | `remote_port`（服务端公网端口） |
| `udp` | UDP 端口映射 | `remote_port` |
| `http` | HTTP 代理，基于域名路由 | `custom_domains`（域名列表） |
| `https` | HTTPS 代理，服务端终止 TLS | `custom_domains` |

---

## TLS 证书

RustProxy 客户端与服务端之间的通信隧道通过 TLS 加密，需正确配置证书以确保安全。

### 自动模式（开发/测试环境）

默认 `auto_cert = true`，首次启动时自动生成自签证书并保存到 `certs/` 目录。后续启动复用已有证书。

此时客户端 `ca_cert` 留空，将跳过证书验证（信任一切连接）。**仅适用于开发/测试环境，存在中间人攻击风险。**

### 推荐模式：拷贝服务端证书到客户端（生产环境）

生产环境应将服务端生成的证书复制到客户端，启用标准 TLS 证书验证：

**第一步：服务端启动后确认证书已生成**

```bash
ls /home/rustproxy/server/certs/
# 应看到 server.crt  server.key
```

**第二步：将服务端证书安全拷贝到客户端机器**

```bash
# 在客户端机器上执行（替换为你的服务端 IP）
mkdir -p /home/rustproxy/client/certs/
scp root@你的服务器IP:/home/rustproxy/server/certs/server.crt /home/rustproxy/client/certs/ca.crt
```

> 只需拷贝 `.crt` 证书文件，**不要**拷贝 `.key` 私钥文件！私钥应仅保存在服务端。

**第三步：客户端配置指定证书路径**

```toml
[client]
# ... 其他配置 ...
ca_cert = "certs/ca.crt"     # 指向刚才拷贝的证书文件
```

配置后客户端将进行标准 TLS 验证，只有持有对应私钥的服务端才能通过握手，有效防止中间人攻击。

### 使用自有 CA / 正式证书

如果你有自己的 CA 或购买了正式证书：

```toml
# 服务端：使用自有证书
[tls]
auto_cert = false
cert_file = "certs/your-cert.crt"
key_file = "certs/your-key.key"

# 客户端：指定 CA 证书
[client]
ca_cert = "/path/to/ca.crt"
```

### 证书配置对照表

| 场景 | 服务端 `auto_cert` | 客户端 `ca_cert` | 安全级别 |
|------|-------------------|-----------------|---------|
| 开发/测试 | `true` | `""` (留空) | ⚠️ 低 — 跳过验证 |
| 生产环境（推荐） | `true` | `"certs/ca.crt"` | ✅ 高 — 标准验证 |
| 自有证书 | `false` | CA 证书路径 | ✅ 高 — 标准验证 |

---

## 常见问题

### 客户端无法连接服务端？

1. 检查服务端隧道端口（默认 7000）是否开放防火墙
2. 检查客户端 `server_addr` 是否正确
3. 检查客户端和服务端的 `token` 是否一致
4. 如果客户端配置了 `ca_cert`，确认证书文件存在且与服务端证书匹配
5. 查看服务端日志：`journalctl -u rustproxy-server -f`

### TLS 证书错误 / 握手失败？

1. 如果使用自签证书且 `ca_cert = ""`，不应出现此错误（跳过验证模式）
2. 如果配置了 `ca_cert`，确认拷贝的是服务端 `certs/server.crt` 而非 `.key` 文件
3. 服务端重新生成证书后（删除 `certs/` 重启），客户端的 `ca.crt` 需要重新拷贝
4. 检查证书文件权限：`ls -la certs/`

### Web 面板无法访问？

1. 检查 Web 面板端口（默认 7500）是否开放防火墙
2. 确认 `web.enable = true`
3. 检查 `web.bind_addr` 是否为 `0.0.0.0`

### 代理端口无法访问？

1. 检查服务端对应远程端口是否开放防火墙
2. 在 Web 面板确认代理规则状态为已激活
3. 确认客户端已连接且收到了代理规则

### 如何查看日志？

```bash
# 服务端
journalctl -u rustproxy-server -f

# 客户端
journalctl -u rustproxy-client -f
```

### 如何更新版本？

```bash
# 1. 下载新版压缩包
# 2. 停止服务
systemctl stop rustproxy-server  # 或 rustproxy-client

# 3. 替换二进制（配置文件保留不覆盖）
cd /home/rustproxy/server
tar xzf rustproxy-server-x86_64-musl.tar.gz rustproxy-server

# 4. 重启服务
systemctl start rustproxy-server
```

### 数据库文件在哪？

默认在工作目录下生成 `rustproxy.db`（SQLite），存储代理规则和客户端状态。可通过 `--db` 参数指定路径：

```bash
./rustproxy-server --db /var/lib/rustproxy/rustproxy.db
```
