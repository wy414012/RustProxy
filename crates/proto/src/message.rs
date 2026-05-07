//! 消息定义
//!
//! 服务端与客户端之间的所有通信消息。
//! 控制消息走 JSON 序列化，数据消息走二进制编码。

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
    /// 客户端请求认证
    Auth(AuthRequest),
    /// 服务端返回认证结果
    AuthResp(AuthResponse),

    // --- 代理管理 ---
    /// 客户端注册代理规则
    RegisterProxy(RegisterProxyRequest),
    /// 服务端确认代理注册
    RegisterProxyResp(RegisterProxyResponse),

    // --- 数据传输 ---
    /// 打开一个新的代理工作连接
    NewWorkConn(NewWorkConnRequest),

    // --- 心跳 ---
    /// 心跳 Ping
    Ping,
    /// 心跳 Pong
    Pong,

    // --- 管理 ---
    /// 服务端要求客户端关闭代理
    CloseProxy(CloseProxyRequest),
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

/// 认证请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub token: String,
    pub version: String,
}

/// 认证响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub success: bool,
    pub message: String,
    pub server_version: String,
}

/// 代理注册请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterProxyRequest {
    pub name: String,
    pub proxy_type: String,
    pub local_ip: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub custom_domains: Vec<String>,
}

/// 代理注册响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterProxyResponse {
    pub success: bool,
    pub message: String,
    pub name: String,
}

/// 新建工作连接请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewWorkConnRequest {
    pub proxy_name: String,
}

/// 关闭代理请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseProxyRequest {
    pub name: String,
}
