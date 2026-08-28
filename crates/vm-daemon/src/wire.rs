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

#[cfg(test)]
mod tests {
    use super::*;

    /// Hard deadline for async tests — a wedged await must FAIL the test with
    /// a named error, never park the build queue.
    async fn bounded<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), fut)
            .await
            .unwrap_or_else(|_| panic!("test deadline exceeded (10s): {what}"))
    }

    /// INVARIANT: Response::ok serializes with success:true and omits the error
    /// field; Response::err sets success:false and omits result — the host
    /// distinguishes them by these exact keys.
    #[test]
    fn response_serialization_shape() {
        let ok = serde_json::to_value(Response::ok(7, serde_json::json!({"x": 1}))).unwrap();
        assert_eq!(ok["id"], 7);
        assert_eq!(ok["success"], true);
        assert_eq!(ok["result"]["x"], 1);
        assert!(!ok.as_object().unwrap().contains_key("error"));

        let err = serde_json::to_value(Response::err(9, "bad")).unwrap();
        assert_eq!(err["id"], 9);
        assert_eq!(err["success"], false);
        assert_eq!(err["error"], "bad");
        assert!(!err.as_object().unwrap().contains_key("result"));
    }

    /// INVARIANT: events serialize event_type under the wire key "type" and
    /// omit every unset optional field — the exact shape the host's GuestEvent
    /// parser expects.
    #[test]
    fn event_serialization_shape() {
        let ev = serde_json::to_value(Event::stdout("p1", "hi".to_string())).unwrap();
        assert_eq!(ev["type"], "stdout");
        assert_eq!(ev["id"], "p1");
        assert_eq!(ev["data"], "hi");
        for absent in ["exit_code", "signal", "message"] {
            assert!(!ev.as_object().unwrap().contains_key(absent), "{absent} should be omitted");
        }

        let exit = serde_json::to_value(Event::exit("p1", 3)).unwrap();
        assert_eq!(exit["type"], "exit");
        assert_eq!(exit["exit_code"], 3);
        assert!(!exit.as_object().unwrap().contains_key("data"));

        let ready = serde_json::to_value(Event::ready()).unwrap();
        assert_eq!(ready["type"], "ready");
        assert_eq!(ready["id"], "");
    }

    /// INVARIANT: requests parse without a params field (params defaults to
    /// None) — method and id alone are a valid request.
    #[test]
    fn request_params_are_optional() {
        let req: Request =
            serde_json::from_value(serde_json::json!({"method": "spawn", "id": 1})).unwrap();
        assert_eq!(req.method, "spawn");
        assert_eq!(req.id, 1);
        assert!(req.params.is_none());

        let with: Request = serde_json::from_value(
            serde_json::json!({"method": "kill", "id": 2, "params": {"id": "p"}}),
        )
        .unwrap();
        assert_eq!(with.params.unwrap()["id"], "p");
    }

    /// INVARIANT: the length-prefixed framing round-trips — what write_message
    /// frames, read_message recovers as the same JSON.
    #[tokio::test]
    async fn wire_round_trip() {
        bounded("wire_round_trip", async {
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (_hr, mut hw) = tokio::io::split(host);
            let (mut gr, _gw) = tokio::io::split(guest);

            let msg =
                serde_json::json!({"method": "readFile", "id": 42, "params": {"path": "/tmp/x"}});
            write_message(&mut hw, &msg).await.unwrap();
            assert_eq!(read_message(&mut gr).await.unwrap(), msg);
        })
        .await;
    }

    /// INVARIANT: a header claiming more than 10 MB is rejected as InvalidData
    /// before allocating or reading the payload.
    #[tokio::test]
    async fn read_rejects_oversized_header() {
        bounded("read_rejects_oversized_header", async {
            let len = ((MAX_MESSAGE_SIZE + 1) as u32).to_be_bytes();
            let mut reader: &[u8] = &len;
            let err = read_message(&mut reader).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        })
        .await;
    }

    /// INVARIANT: write_message refuses payloads over 10 MB instead of framing
    /// a message the host will reject.
    #[tokio::test]
    async fn write_rejects_oversized_payload() {
        bounded("write_rejects_oversized_payload", async {
            let big = "a".repeat(MAX_MESSAGE_SIZE);
            let mut sink = tokio::io::sink();
            let err = write_message(&mut sink, &big).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        })
        .await;
    }
}
