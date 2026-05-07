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
    RegisterProxy = 0x03,
    RegisterProxyResp = 0x04,
    NewWorkConn = 0x05,
    Data = 0x06,
    Ping = 0x07,
    Pong = 0x08,
    CloseProxy = 0x09,
}

impl TryFrom<u8> for MessageType {
    type Error = FrameError;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0x01 => Ok(MessageType::Auth),
            0x02 => Ok(MessageType::AuthResp),
            0x03 => Ok(MessageType::RegisterProxy),
            0x04 => Ok(MessageType::RegisterProxyResp),
            0x05 => Ok(MessageType::NewWorkConn),
            0x06 => Ok(MessageType::Data),
            0x07 => Ok(MessageType::Ping),
            0x08 => Ok(MessageType::Pong),
            0x09 => Ok(MessageType::CloseProxy),
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
}

/// 帧头大小: 4 (length) + 1 (type) + 2 (reserved) = 7 bytes
const HEADER_SIZE: usize = 7;

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
                    ControlMessage::RegisterProxy(req) => {
                        (MessageType::RegisterProxy, serde_json::to_vec(req)?)
                    }
                    ControlMessage::RegisterProxyResp(resp) => {
                        (MessageType::RegisterProxyResp, serde_json::to_vec(resp)?)
                    }
                    ControlMessage::NewWorkConn(req) => {
                        (MessageType::NewWorkConn, serde_json::to_vec(req)?)
                    }
                    ControlMessage::Ping => (MessageType::Ping, Vec::new()),
                    ControlMessage::Pong => (MessageType::Pong, Vec::new()),
                    ControlMessage::CloseProxy(req) => {
                        (MessageType::CloseProxy, serde_json::to_vec(req)?)
                    }
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
            MessageType::RegisterProxy => Message::Control(ControlMessage::RegisterProxy(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::RegisterProxyResp => Message::Control(ControlMessage::RegisterProxyResp(
                serde_json::from_slice(&payload)?,
            )),
            MessageType::NewWorkConn => Message::Control(ControlMessage::NewWorkConn(
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
            MessageType::CloseProxy => Message::Control(ControlMessage::CloseProxy(
                serde_json::from_slice(&payload)?,
            )),
        };

        Ok(Some(msg))
    }
}
