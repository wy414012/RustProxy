# Changelog

---

## [Unreleased]

### Fixed

- 编辑代理规则时仅修改 `local_addr` 而 `remote_port` 不变导致报错 (#3)
  - `stop_listener` 在 `abort()` 后等待任务实际结束再返回，确保端口释放后再绑定新监听器
  - `remote_port` 验证仅对 TCP/UDP 类型要求非零，HTTP/HTTPS 的 `remote_port: 0` 为合法值
  - 前端编辑 HTTP/HTTPS 规则时不发送 `remote_port` 字段，实现真正的部分更新
- 代理规则热刷新不及时，修改/删除代理规则后需重启服务端才生效 (#2)
  - 修复 `update_proxy` 用更新后的规则调用删除回调，导致旧监听器/域名路由无法正确清理
  - 回调类型从同步改为异步，等待监听器实际启停完成，避免端口冲突
  - 新增 `Starting`/`Stopping`/`Error` 中间状态，操作按钮在状态转换期间自动禁用
  - 前端状态标签中文化（启动中/运行中/停止中/已停止/错误）

---

## [0.1.4] - 2025-05-08

**首个开源发行版本。** 之前所有提交与变更均属于初代开发阶段，0.1.4 为首个面向公众发布的稳定版本。

### Added

- SNI 域名配置支持（HTTPS 代理可指定 SNI 域名）
- `hash-password` 子命令，方便生成 bcrypt 密码哈希
- `jwt_secret` 配置项，JWT 密钥独立于客户端 Token
- 安全机制文档说明

### Fixed

- 综合安全加固 — JWT 认证 / bcrypt 密码哈希 / API 速率限制 / CORS 跨域控制 / 输入验证
- 帧协议最大长度限制，防止恶意超大帧攻击

---

## [0.1.3] - 2025-05-06

### Added

- PROXY Protocol 注入支持，可将真实客户端 IP 传递给后端服务

### Fixed

- 管理面板连接数显示为 0 的 Bug

---

## [0.1.2] - 2025-05-04

### Added

- 多架构构建和发布流程（amd64 + arm64，静态链接 musl）
- 代理配置表单交互优化与字段显示改进
- UDP / HTTP / HTTPS 代理类型及流量统计
- Web 管理面板和 WebSocket 实时状态推送
- TCP 代理双向数据转发
- 服务端集中管理代理规则架构

### Changed

- 项目元数据和配置文件更新
- 规范代码以符合零警告原则

### Infrastructure

- 多架构 CI 配置（cargo check / test / fmt / clippy / cross-build）
- CNB 工作区配置
- 发布流程自动化

---

[Unreleased]: https://cnb.cool/emchaye/RustProxy/compare/0.1.4...HEAD
[0.1.4]: https://cnb.cool/emchaye/RustProxy/releases/tag/0.1.4
[0.1.3]: https://cnb.cool/emchaye/RustProxy/releases/tag/0.1.3
[0.1.2]: https://cnb.cool/emchaye/RustProxy/releases/tag/0.1.2
