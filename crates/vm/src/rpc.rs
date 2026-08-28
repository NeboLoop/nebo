//! Host-side RPC client for communicating with the guest daemon.
//!
//! Wire format: `[4 bytes: u32 BE length][N bytes: UTF-8 JSON]`
//!
//! Two connections:
//! 1. **Request/Response** — multiplexed with request IDs
//! 2. **Event stream** — guest pushes stdout/stderr/exit/status events

use crate::error::{VmError, VmResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, warn};

/// Maximum message size: 10 MB.
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Default RPC timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

// ── Wire Protocol ──────────────────────────────────────────────────

/// Write a length-prefixed JSON message to a stream.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &impl Serialize,
) -> VmResult<()> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(VmError::MessageTooLarge {
            size: payload.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    let len = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed JSON message from a stream.
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> VmResult<serde_json::Value> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > MAX_MESSAGE_SIZE {
        return Err(VmError::MessageTooLarge {
            size: len,
            max: MAX_MESSAGE_SIZE,
        });
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    let msg = serde_json::from_slice(&payload)?;
    Ok(msg)
}

// ── Message Types ──────────────────────────────────────────────────

/// Request sent from host to guest.
#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub method: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Response from guest to host.
#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub success: bool,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Event pushed from guest to host on the event stream.
#[derive(Debug, Clone, Deserialize)]
pub struct GuestEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub oom_kill_count: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub fatal: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
}

// ── Spawn Parameters ───────────────────────────────────────────────

/// Parameters for spawning a process inside the VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnParams {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub one_shot: bool,
}

/// Result of a spawn request.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnResult {
    pub process_id: String,
}

// ── File Transfer Parameters ───────────────────────────────────────

/// Write a file inside the VM.
#[derive(Debug, Serialize)]
pub struct WriteFileParams {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub append: bool,
}

/// Copy files from VM to host.
#[derive(Debug, Serialize)]
pub struct CopyOutParams {
    /// Paths inside the VM to copy.
    pub src_paths: Vec<String>,
    /// Destination directory on the host.
    pub dest_dir: String,
}

/// Result of a copy-out operation.
#[derive(Debug, Deserialize)]
pub struct CopyOutResult {
    /// Files successfully copied.
    pub copied: Vec<CopiedFile>,
    /// Files that failed: (vm_path, error).
    pub errors: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
pub struct CopiedFile {
    pub vm_path: String,
    pub host_path: String,
    pub size_bytes: u64,
}

// ── VM Client ──────────────────────────────────────────────────────

/// Pending request awaiting a response.
struct PendingRequest {
    tx: oneshot::Sender<RpcResponse>,
    #[allow(dead_code)] // retained for debug logging
    method: String,
}

/// Outbound message to be sent to the guest.
struct OutboundMessage {
    payload: Vec<u8>,
}

/// Host-side RPC client for the guest VM daemon.
///
/// Uses an mpsc channel for writing (avoids dyn trait issues) and
/// multiplexed request IDs for concurrent RPC calls.
pub struct VmClient {
    /// Channel to send outbound messages to the writer task.
    write_tx: mpsc::UnboundedSender<OutboundMessage>,
    /// Pending requests keyed by request ID.
    pending: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    /// Monotonically increasing request ID.
    next_id: AtomicU64,
    /// Channel for events received from the guest.
    event_tx: mpsc::UnboundedSender<GuestEvent>,
}

impl VmClient {
    /// Create a new VM client (not yet connected).
    ///
    /// Returns the client and an event receiver for guest events.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<GuestEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        // Create the write channel — messages will be forwarded to the
        // actual stream once connect() is called.
        let (write_tx, _write_rx) = mpsc::unbounded_channel();

        let client = Self {
            write_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        };
        (client, event_rx)
    }

    /// Connect to the guest daemon over a read/write stream.
    ///
    /// Spawns background tasks for reading responses/events and writing
    /// outbound messages.
    pub fn connect<R, W>(
        &mut self,
        reader: R,
        writer: W,
    ) where
        R: AsyncReadExt + Unpin + Send + 'static,
        W: AsyncWriteExt + Unpin + Send + 'static,
    {
        let pending = self.pending.clone();
        let event_tx = self.event_tx.clone();

        // Read loop — routes incoming messages to pending requests or event channel
        tokio::spawn(async move {
            let mut reader = reader;
            loop {
                match read_message(&mut reader).await {
                    Ok(msg) => {
                        if msg.get("success").is_some() {
                            if let Ok(resp) = serde_json::from_value::<RpcResponse>(msg) {
                                let mut map = pending.lock().await;
                                if let Some(req) = map.remove(&resp.id) {
                                    let _ = req.tx.send(resp);
                                } else {
                                    warn!(id = resp.id, "orphaned RPC response");
                                }
                            }
                        } else if let Ok(event) = serde_json::from_value::<GuestEvent>(msg) {
                            let _ = event_tx.send(event);
                        }
                    }
                    Err(VmError::Io(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        debug!("guest connection closed");
                        break;
                    }
                    Err(e) => {
                        error!(%e, "error reading from guest");
                        break;
                    }
                }
            }
        });

        // Write loop — sends outbound messages from the channel to the stream
        let (new_tx, mut write_rx) = mpsc::unbounded_channel::<OutboundMessage>();
        self.write_tx = new_tx;

        tokio::spawn(async move {
            let mut writer = writer;
            while let Some(msg) = write_rx.recv().await {
                let len = (msg.payload.len() as u32).to_be_bytes();
                if writer.write_all(&len).await.is_err() {
                    break;
                }
                if writer.write_all(&msg.payload).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });
    }

    /// Send an RPC request and await the response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> VmResult<serde_json::Value> {
        self.request_with_timeout(method, params, DEFAULT_TIMEOUT_SECS)
            .await
    }

    /// Send an RPC request with a custom timeout.
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        timeout_secs: u64,
    ) -> VmResult<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = RpcRequest {
            method: method.to_string(),
            id,
            params,
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(
                id,
                PendingRequest {
                    tx,
                    method: method.to_string(),
                },
            );
        }

        // Serialize and send via the write channel
        let payload = serde_json::to_vec(&req)?;
        self.write_tx
            .send(OutboundMessage { payload })
            .map_err(|_| VmError::GuestNotConnected)?;

        // Await response with timeout
        let timeout = tokio::time::Duration::from_secs(timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => {
                if resp.success {
                    Ok(resp.result.unwrap_or(serde_json::Value::Null))
                } else {
                    Err(VmError::RpcError(
                        resp.error.unwrap_or_else(|| "unknown error".to_string()),
                    ))
                }
            }
            Ok(Err(_)) => Err(VmError::GuestNotConnected),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(VmError::RpcTimeout {
                    method: method.to_string(),
                    timeout_secs,
                })
            }
        }
    }

    // ── Convenience Methods ────────────────────────────────────────

    /// Spawn a process inside the VM.
    pub async fn spawn(&self, params: SpawnParams) -> VmResult<SpawnResult> {
        let result = self
            .request("spawn", Some(serde_json::to_value(&params)?))
            .await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Kill a process inside the VM.
    pub async fn kill(&self, process_id: &str, signal: &str) -> VmResult<()> {
        self.request(
            "kill",
            Some(serde_json::json!({ "id": process_id, "signal": signal })),
        )
        .await?;
        Ok(())
    }

    /// Write to a process's stdin.
    pub async fn write_stdin(&self, process_id: &str, data: &str) -> VmResult<()> {
        self.request(
            "writeStdin",
            Some(serde_json::json!({ "id": process_id, "data": data })),
        )
        .await?;
        Ok(())
    }

    /// Check if a process is running.
    pub async fn is_process_running(&self, process_id: &str) -> VmResult<bool> {
        let result = self
            .request(
                "isProcessRunning",
                Some(serde_json::json!({ "id": process_id })),
            )
            .await?;
        Ok(result
            .get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Read a file from inside the VM.
    pub async fn read_file(&self, path: &str) -> VmResult<String> {
        let result = self
            .request("readFile", Some(serde_json::json!({ "path": path })))
            .await?;
        result
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| VmError::RpcError("readFile returned no content".to_string()))
    }

    /// Write a file inside the VM.
    pub async fn write_file(&self, params: WriteFileParams) -> VmResult<()> {
        self.request("writeFile", Some(serde_json::to_value(&params)?))
            .await?;
        Ok(())
    }

    /// Copy files from the VM to the host filesystem.
    ///
    /// The guest daemon reads the files and streams them back over RPC.
    /// The host writes them to `dest_dir`. The VM never writes to the
    /// host directly — the host always pulls.
    pub async fn copy_out(&self, params: CopyOutParams) -> VmResult<CopyOutResult> {
        let dest_dir = params.dest_dir.clone();

        let result = self
            .request_with_timeout(
                "copyOut",
                Some(serde_json::to_value(&params)?),
                120,
            )
            .await?;

        let files: Vec<FilePayload> = serde_json::from_value(
            result
                .get("files")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
        )?;

        let mut copied = Vec::new();
        let mut errors = Vec::new();

        for file in files {
            let relative = file.path.trim_start_matches('/');
            let host_path = std::path::Path::new(&dest_dir).join(relative);

            if let Some(parent) = host_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    errors.push((file.path, e.to_string()));
                    continue;
                }
            }

            match base64_decode(&file.content_base64) {
                Ok(bytes) => {
                    let size = bytes.len() as u64;
                    match std::fs::write(&host_path, &bytes) {
                        Ok(()) => {
                            copied.push(CopiedFile {
                                vm_path: file.path,
                                host_path: host_path.to_string_lossy().to_string(),
                                size_bytes: size,
                            });
                        }
                        Err(e) => errors.push((file.path, e.to_string())),
                    }
                }
                Err(e) => errors.push((file.path, format!("base64 decode: {e}"))),
            }
        }

        Ok(CopyOutResult { copied, errors })
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // deserialized from wire protocol
struct FilePayload {
    path: String,
    content_base64: String,
    size_bytes: u64,
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const TABLE: [u8; 256] = {
        let mut t = [255u8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            t[(b'a' + i) as usize] = i + 26;
            i += 1;
        }
        let mut j = 0u8;
        while j < 10 {
            t[(b'0' + j) as usize] = j + 52;
            j += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t[b'=' as usize] = 0;
        t
    };

    let filtered: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
        .collect();

    if filtered.len() % 4 != 0 {
        return Err("invalid base64 length".to_string());
    }

    let mut output = Vec::with_capacity(filtered.len() * 3 / 4);
    for chunk in filtered.chunks(4) {
        let a = TABLE[chunk[0] as usize];
        let b = TABLE[chunk[1] as usize];
        if a == 255 || b == 255 {
            return Err("invalid base64 character".to_string());
        }
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = TABLE[chunk[2] as usize];
            output.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = TABLE[chunk[3] as usize];
                output.push((c << 6) | d);
            }
        }
    }

    Ok(output)
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

    /// INVARIANT: the wire framing round-trips — a message written with
    /// write_message is read back identically by read_message.
    #[tokio::test]
    async fn wire_round_trip() {
        bounded("wire_round_trip", async {
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (_hr, mut hw) = tokio::io::split(host);
            let (mut gr, _gw) = tokio::io::split(guest);

            let msg = serde_json::json!({"method": "spawn", "id": 7, "params": {"x": [1, 2, 3]}});
            write_message(&mut hw, &msg).await.unwrap();
            let read = read_message(&mut gr).await.unwrap();
            assert_eq!(read, msg);
        })
        .await;
    }

    /// INVARIANT: a length header claiming more than 10 MB is rejected before
    /// any payload is read (MessageTooLarge, not an allocation).
    #[tokio::test]
    async fn read_rejects_oversized_header() {
        bounded("read_rejects_oversized_header", async {
            let len = (MAX_MESSAGE_SIZE as u32 + 1).to_be_bytes();
            let mut reader: &[u8] = &len;
            let err = read_message(&mut reader).await.unwrap_err();
            assert!(matches!(err, VmError::MessageTooLarge { size, max }
                if size == MAX_MESSAGE_SIZE + 1 && max == MAX_MESSAGE_SIZE));
        })
        .await;
    }

    /// INVARIANT: write_message refuses payloads over 10 MB instead of framing
    /// a message the other side will reject.
    #[tokio::test]
    async fn write_rejects_oversized_payload() {
        bounded("write_rejects_oversized_payload", async {
            let big = "a".repeat(MAX_MESSAGE_SIZE);
            let mut sink = tokio::io::sink();
            let err = write_message(&mut sink, &big).await.unwrap_err();
            assert!(matches!(err, VmError::MessageTooLarge { .. }));
        })
        .await;
    }

    /// INVARIANT: GuestEvent deserializes the guest daemon's event shape —
    /// "type" maps to event_type and every optional field defaults instead of
    /// failing on absence.
    #[test]
    fn guest_event_parses_daemon_shape() {
        let ev: GuestEvent =
            serde_json::from_value(serde_json::json!({"type": "exit", "id": "p1", "exit_code": 3}))
                .unwrap();
        assert_eq!(ev.event_type, "exit");
        assert_eq!(ev.id, "p1");
        assert_eq!(ev.exit_code, Some(3));
        assert_eq!(ev.signal, None);
        assert_eq!(ev.data, None);

        let ready: GuestEvent =
            serde_json::from_value(serde_json::json!({"type": "ready", "id": ""})).unwrap();
        assert_eq!(ready.event_type, "ready");
    }

    /// INVARIANT: RpcResponse tolerates minimal responses — result and error
    /// default to None rather than being required fields.
    #[test]
    fn rpc_response_parses_minimal() {
        let resp: RpcResponse =
            serde_json::from_value(serde_json::json!({"id": 1, "success": true})).unwrap();
        assert_eq!(resp.id, 1);
        assert!(resp.success);
        assert!(resp.result.is_none());
        assert!(resp.error.is_none());
    }

    /// INVARIANT: SpawnParams omits unset optional fields and empty allowlists
    /// on the wire (the guest applies its own defaults), while required fields
    /// and one_shot are always present.
    #[test]
    fn spawn_params_skip_serialization() {
        let params = SpawnParams {
            id: "s1".to_string(),
            name: "n".to_string(),
            command: "echo".to_string(),
            args: vec![],
            cwd: None,
            env: None,
            timeout_secs: None,
            allowed_domains: vec![],
            one_shot: true,
        };
        let v = serde_json::to_value(&params).unwrap();
        let obj = v.as_object().unwrap();
        for absent in ["cwd", "env", "timeout_secs", "allowed_domains"] {
            assert!(!obj.contains_key(absent), "{absent} should be skipped");
        }
        assert_eq!(v["one_shot"], true);
        assert_eq!(v["command"], "echo");
    }

    /// INVARIANT: base64_decode decodes standard base64 (with padding and with
    /// embedded whitespace) and rejects bad characters and bad lengths.
    #[test]
    fn base64_decode_behavior() {
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aA==").unwrap(), b"h");
        assert_eq!(base64_decode("Zm9v\nYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert!(base64_decode("a!==").is_err(), "invalid character");
        assert!(base64_decode("abc").is_err(), "length not a multiple of 4");
    }

    /// INVARIANT: a request on a client that was never connected fails fast
    /// with GuestNotConnected instead of hanging until timeout.
    #[tokio::test]
    async fn request_without_connection_fails_fast() {
        bounded("request_without_connection_fails_fast", async {
            let (client, _events) = VmClient::new();
            let err = client.request("ping", None).await.unwrap_err();
            assert!(matches!(err, VmError::GuestNotConnected));
        })
        .await;
    }

    /// INVARIANT: requests are multiplexed by id — a response with the matching
    /// id resolves the request with its result payload.
    #[tokio::test]
    async fn request_response_round_trip() {
        bounded("request_response_round_trip", async {
            let (mut client, _events) = VmClient::new();
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (hr, hw) = tokio::io::split(host);
            let (mut gr, mut gw) = tokio::io::split(guest);
            client.connect(hr, hw);

            let guest_task = tokio::spawn(async move {
                let msg = read_message(&mut gr).await.unwrap();
                assert_eq!(msg["method"], "ping");
                let id = msg["id"].as_u64().unwrap();
                write_message(
                    &mut gw,
                    &serde_json::json!({"id": id, "success": true, "result": {"pong": true}}),
                )
                .await
                .unwrap();
                (gr, gw)
            });

            let result = client
                .request("ping", Some(serde_json::json!({"x": 1})))
                .await
                .unwrap();
            assert_eq!(result["pong"], true);
            guest_task.await.unwrap();
        })
        .await;
    }

    /// INVARIANT: a guest failure response surfaces as RpcError carrying the
    /// guest's error string.
    #[tokio::test]
    async fn error_response_maps_to_rpc_error() {
        bounded("error_response_maps_to_rpc_error", async {
            let (mut client, _events) = VmClient::new();
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (hr, hw) = tokio::io::split(host);
            let (mut gr, mut gw) = tokio::io::split(guest);
            client.connect(hr, hw);

            let guest_task = tokio::spawn(async move {
                let msg = read_message(&mut gr).await.unwrap();
                let id = msg["id"].as_u64().unwrap();
                write_message(
                    &mut gw,
                    &serde_json::json!({"id": id, "success": false, "error": "boom"}),
                )
                .await
                .unwrap();
                (gr, gw)
            });

            let err = client.request("kill", None).await.unwrap_err();
            assert!(matches!(err, VmError::RpcError(ref m) if m == "boom"));
            guest_task.await.unwrap();
        })
        .await;
    }

    /// INVARIANT: a silent guest trips the RPC timeout, reporting the method
    /// and the timeout that was exceeded.
    #[tokio::test]
    async fn silent_guest_times_out() {
        bounded("silent_guest_times_out", async {
            let (mut client, _events) = VmClient::new();
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (hr, hw) = tokio::io::split(host);
            let (_gr, _gw) = tokio::io::split(guest); // keep the guest side open, silent
            client.connect(hr, hw);

            let err = client
                .request_with_timeout("ping", None, 0)
                .await
                .unwrap_err();
            assert!(matches!(err, VmError::RpcTimeout { ref method, timeout_secs }
                if method == "ping" && timeout_secs == 0));
        })
        .await;
    }

    /// INVARIANT: messages without a "success" field are events, routed to the
    /// event channel rather than the request map.
    #[tokio::test]
    async fn guest_events_route_to_event_channel() {
        bounded("guest_events_route_to_event_channel", async {
            let (mut client, mut events) = VmClient::new();
            let (host, guest) = tokio::io::duplex(64 * 1024);
            let (hr, hw) = tokio::io::split(host);
            let (_gr, mut gw) = tokio::io::split(guest);
            client.connect(hr, hw);

            write_message(
                &mut gw,
                &serde_json::json!({"type": "stdout", "id": "p1", "data": "hi"}),
            )
            .await
            .unwrap();

            let ev = events.recv().await.unwrap();
            assert_eq!(ev.event_type, "stdout");
            assert_eq!(ev.id, "p1");
            assert_eq!(ev.data.as_deref(), Some("hi"));
        })
        .await;
    }
}
