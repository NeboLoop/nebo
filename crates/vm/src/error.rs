use thiserror::Error;

pub type VmResult<T> = Result<T, VmError>;

#[derive(Debug, Error)]
pub enum VmError {
    #[error("VM not running")]
    NotRunning,

    #[error("VM already running")]
    AlreadyRunning,

    #[error("VM boot timed out after {0}s")]
    BootTimeout(u64),

    #[error("guest daemon not connected")]
    GuestNotConnected,

    #[error("RPC request timed out: {method} ({timeout_secs}s)")]
    RpcTimeout { method: String, timeout_secs: u64 },

    #[error("RPC error from guest: {0}")]
    RpcError(String),

    #[error("wire protocol error: {0}")]
    WireError(String),

    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge { size: usize, max: usize },

    #[error("process {id} not found in VM")]
    ProcessNotFound { id: String },

    #[error("file transfer failed: {0}")]
    TransferFailed(String),

    #[error("file not found in VM: {path}")]
    FileNotFound { path: String },

    #[error("platform not supported: {0}")]
    PlatformNotSupported(String),

    #[error("Swift helper compilation failed: {0}")]
    SwiftCompilationFailed(String),

    #[error("VM image not found: {0}")]
    ImageNotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
