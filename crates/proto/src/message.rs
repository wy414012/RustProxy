//! 消息定义
//!
//! 服务端与客户端之间的所有通信消息。
//! 控制消息走 JSON 序列化，数据消息走二进制编码。
//!
//! 核心架构：服务端集中管理代理规则，客户端只接收指令。
//! - 客户端连接后发送 Auth（携带 client_id + token）
//! - 服务端认证成功后，推送该 client_id 的所有代理规则（ServerAssignProxy）
//! - 管理员通过 Web 面板增删代理规则时，服务端实时推送变更

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};

/// 顶层消息枚举
#[derive(Debug, Clone)]
pub enum Message {
    /// 控制消息（JSON 序列化）
    Control(ControlMessage),
    /// 数据消息（二进制编码）
    Data(DataMessage),
}

/// 控制消息（可 JSON 序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    // --- 连接阶段 ---
    /// 客户端请求认证（携带 client_id + token）
    Auth(AuthRequest),
    /// 服务端返回认证结果
    AuthResp(AuthResponse),

    // --- 代理管理（服务端→客户端） ---
    /// 服务端向客户端推送代理规则（新增或更新）
    ServerAssignProxy(ServerAssignProxyRequest),
    /// 服务端要求客户端关闭代理
    ServerCloseProxy(ServerCloseProxyRequest),

    // --- 数据传输 ---
    /// 服务端通知客户端打开一个新的代理工作连接
    NewWorkConn(NewWorkConnRequest),
    /// 客户端确认工作连接已就绪
    NewWorkConnResp(NewWorkConnResponse),

    // --- 心跳 ---
    /// 心跳 Ping
    Ping,
    /// 心跳 Pong
    Pong,
}

/// 数据消息（二进制帧，不走 JSON 序列化）
#[derive(Debug, Clone)]
pub struct DataMessage {
    pub conn_id: u64,
    pub data: Bytes,
}

impl DataMessage {
    /// 编码为字节: 8 bytes conn_id + data
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(8 + self.data.len());
        buf.extend_from_slice(&self.conn_id.to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf.freeze()
    }

    /// 从字节解码
    pub fn decode(src: &[u8]) -> Option<Self> {
        if src.len() < 8 {
            return None;
        }
        let conn_id = u64::from_be_bytes(src[..8].try_into().ok()?);
        let data = Bytes::copy_from_slice(&src[8..]);
        Some(Self { conn_id, data })
    }
}

// ============================================================
// 认证
// ============================================================

/// 认证请求（客户端 → 服务端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    /// 客户端唯一标识
    pub client_id: String,
    /// 鉴权 Token
    pub token: String,
    /// 客户端版本
    pub version: String,
}

/// 认证响应（服务端 → 客户端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub success: bool,
    pub message: String,
    pub server_version: String,
}

// ============================================================
// 代理管理（服务端 → 客户端）
// ============================================================

/// 服务端推送代理规则（新增或更新）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerAssignProxyRequest {
    /// 代理规则名称
    pub name: String,
    /// 代理类型: tcp / udp / http / https
    pub proxy_type: String,
    /// 客户端本地服务地址
    pub local_ip: String,
    /// 客户端本地服务端口
    pub local_port: u16,
    /// 服务端公网端口（TCP/UDP 有效）
    pub remote_port: u16,
    /// 自定义域名（HTTP/HTTPS 有效）
    #[serde(default)]
    pub custom_domains: Vec<String>,
    /// PROXY Protocol 版本: "" / "v1" / "v2"
    #[serde(default)]
    pub proxy_protocol: String,
}

/// 服务端要求客户端关闭代理
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCloseProxyRequest {
    /// 代理规则名称
    pub name: String,
}

// ============================================================
// 数据传输
// ============================================================

/// 新建工作连接请求（服务端 → 客户端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewWorkConnRequest {
    /// 代理规则名称
    pub proxy_name: String,
    /// 连接 ID（服务端分配）
    pub conn_id: u64,
    /// 用户真实 IP 地址（格式: "ip:port"）
    #[serde(default)]
    pub user_addr: Option<String>,
}

/// 新建工作连接响应（客户端 → 服务端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewWorkConnResponse {
    /// 代理规则名称
    pub proxy_name: String,
    /// 连接 ID
    pub conn_id: u64,
    /// 是否成功
    pub success: bool,
}
