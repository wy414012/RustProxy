//! RustProxy Proto — 通信协议定义
//!
//! 定义服务端与客户端之间的消息格式、帧编解码等。

pub mod frame;
pub mod message;

pub use message::Message;
