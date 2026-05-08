//! 帧编解码
//!
//! 基于 Length-delimited 的帧协议：
//!
//! ```text
//! +--------+--------+----------+-----------+
//! | 4 byte | 1 byte | 2 byte   | N bytes   |
//! | length | type   | reserved | payload   |
//! +--------+--------+----------+-----------+
//! ```
//!
//! - length: payload 长度（大端序）
//! - type:   消息类型标识
//! - reserved: 保留字段
//! - payload: 控制消息为 JSON，数据消息为二进制

use bytes::{Buf, BytesMut};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};

use crate::message::{ControlMessage, DataMessage, Message};

/// 消息类型标识
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Auth = 0x01,
    AuthResp = 0x02,
    ServerAssignProxy = 0x03,
    ServerCloseProxy = 0x04,
    NewWorkConn = 0x05,
    NewWorkConnResp = 0x06,
    Data = 0x07,
    Ping = 0x08,
    Pong = 0x09,
}

impl TryFrom<u8> for MessageType {
    type Error = FrameError;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0x01 => Ok(MessageType::Auth),
            0x02 => Ok(MessageType::AuthResp),
            0x03 => Ok(MessageType::ServerAssignProxy),
            0x04 => Ok(MessageType::ServerCloseProxy),
            0x05 => Ok(MessageType::NewWorkConn),
            0x06 => Ok(MessageType::NewWorkConnResp),
            0x07 => Ok(MessageType::Data),
            0x08 => Ok(MessageType::Ping),
            0x09 => Ok(MessageType::Pong),
            _ => Err(FrameError::UnknownType(value)),
        }
    }
}

/// 帧编解码错误
#[derive(Error, Debug)]
pub enum FrameError {
    #[error("未知消息类型: {0}")]
    UnknownType(u8),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("帧长度超过最大限制: {0} > {MAX_FRAME_SIZE}")]
    FrameTooLarge(usize),
}

/// 帧头大小: 4 (length) + 1 (type) + 2 (reserved) = 7 bytes
const HEADER_SIZE: usize = 7;

/// 单帧最大 payload 长度（64 MiB）
///
/// 超过此长度的帧将被拒绝并断开连接，防止恶意客户端通过超大 length 值触发内存耗尽攻击。
/// 正常代理流量中，单帧 unlikely 超过几 MB；如需传输更大数据，应在应用层分片。
const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

/// RustProxy 帧编解码器
#[derive(Debug, Default)]
pub struct FrameCodec;

impl Encoder<Message> for FrameCodec {
    type Error = FrameError;

    fn encode(
        &mut self,
        item: Message,
        dst: &mut BytesMut,
    ) -> std::result::Result<(), Self::Error> {
        match item {
            Message::Control(ctrl) => {
                let (msg_type, payload) = match &ctrl {
                    ControlMessage::Auth(req) => (MessageType::Auth, serde_json::to_vec(req)?),
                    ControlMessage::AuthResp(resp) => {
                        (MessageType::AuthResp, serde_json::to_vec(resp)?)
                    }
                    ControlMessage::ServerAssignProxy(req) => {
                        (MessageType::ServerAssignProxy, serde_json::to_vec(req)?)
                    }
                    ControlMessage::ServerCloseProxy(req) => {
                        (MessageType::ServerCloseProxy, serde_json::to_vec(req)?)
                    }
                    ControlMessage::NewWorkConn(req) => {
                        (MessageType::NewWorkConn, serde_json::to_vec(req)?)
                    }
                    ControlMessage::NewWorkConnResp(resp) => {
                        (MessageType::NewWorkConnResp, serde_json::to_vec(resp)?)
                    }
                    ControlMessage::Ping => (MessageType::Ping, Vec::new()),
                    ControlMessage::Pong => (MessageType::Pong, Vec::new()),
                };

                let len = payload.len() as u32;
                dst.reserve(HEADER_SIZE + payload.len());
                dst.extend_from_slice(&len.to_be_bytes());
                dst.extend_from_slice(&[msg_type as u8]);
                dst.extend_from_slice(&[0u8, 0u8]); // reserved
                dst.extend_from_slice(&payload);
            }
            Message::Data(data_msg) => {
                let payload = data_msg.encode();
                let len = payload.len() as u32;
                dst.reserve(HEADER_SIZE + payload.len());
                dst.extend_from_slice(&len.to_be_bytes());
                dst.extend_from_slice(&[MessageType::Data as u8]);
                dst.extend_from_slice(&[0u8, 0u8]); // reserved
                dst.extend_from_slice(&payload);
            }
        }

        Ok(())
    }
}

impl Decoder for FrameCodec {
    type Item = Message;
    type Error = FrameError;

    fn decode(
        &mut self,
        src: &mut BytesMut,
    ) -> std::result::Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_SIZE {
            return Ok(None);
        }

        let mut header = src.clone();
        let len = header.get_u32() as usize;
        let msg_type_val = header.get_u8();
        let _reserved = header.get_u16();

        // 防御性检查：拒绝超大帧，防止 DoS 攻击
        if len > MAX_FRAME_SIZE {
            return Err(FrameError::FrameTooLarge(len));
        }

        if src.len() < HEADER_SIZE + len {
            return Ok(None);
        }

        let msg_type = MessageType::try_from(msg_type_val)?;
        src.advance(HEADER_SIZE);
        let payload = src.split_to(len);

        let msg = match msg_type {
            MessageType::Auth => {
                Message::Control(ControlMessage::Auth(serde_json::from_slice(&payload)?))
            }
            MessageType::AuthResp => {
                Message::Control(ControlMessage::AuthResp(serde_json::from_slice(&payload)?))
            }
            MessageType::ServerAssignProxy => Message::Control(ControlMessage::ServerAssignProxy(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::ServerCloseProxy => Message::Control(ControlMessage::ServerCloseProxy(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::NewWorkConn => Message::Control(ControlMessage::NewWorkConn(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::NewWorkConnResp => Message::Control(ControlMessage::NewWorkConnResp(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::Data => {
                if let Some(data_msg) = DataMessage::decode(&payload) {
                    Message::Data(data_msg)
                } else {
                    return Ok(None);
                }
            }
            MessageType::Ping => Message::Control(ControlMessage::Ping),
            MessageType::Pong => Message::Control(ControlMessage::Pong),
        };

        Ok(Some(msg))
    }
}
