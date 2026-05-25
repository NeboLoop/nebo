//! Wire protocol: length-prefixed JSON framing.
//!
//! Format: [4 bytes: u32 BE length][N bytes: UTF-8 JSON]
//! Shared between host (nebo-vm crate) and guest (this crate).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum message size: 10 MB.
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Write a length-prefixed JSON message.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &impl Serialize,
) -> std::io::Result<()> {
    let payload = serde_json::to_vec(msg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("message too large: {} bytes", payload.len()),
        ));
    }
    let len = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed JSON message.
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<serde_json::Value> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > MAX_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("message too large: {len} bytes"),
        ));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })
}

// ── Message Types ──────────────────────────────────────────────────

/// Incoming request from the host.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub method: String,
    pub id: u64,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Outgoing response to the host.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: u64,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            success: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            success: false,
            result: None,
            error: Some(error.into()),
        }
    }
}

/// Outgoing event (pushed to host without a request).
#[derive(Debug, Serialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub event_type: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Event {
    pub fn stdout(id: &str, data: String) -> Self {
        Self {
            event_type: "stdout".to_string(),
            id: id.to_string(),
            data: Some(data),
            exit_code: None,
            signal: None,
            message: None,
        }
    }

    pub fn stderr(id: &str, data: String) -> Self {
        Self {
            event_type: "stderr".to_string(),
            id: id.to_string(),
            data: Some(data),
            exit_code: None,
            signal: None,
            message: None,
        }
    }

    pub fn exit(id: &str, code: i32) -> Self {
        Self {
            event_type: "exit".to_string(),
            id: id.to_string(),
            data: None,
            exit_code: Some(code),
            signal: None,
            message: None,
        }
    }

    pub fn ready() -> Self {
        Self {
            event_type: "ready".to_string(),
            id: String::new(),
            data: None,
            exit_code: None,
            signal: None,
            message: None,
        }
    }
}
