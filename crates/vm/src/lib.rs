//! # nebo-vm
//!
//! Lightweight VM sandbox for isolated code execution. Provides a hardware-level
//! isolation boundary between the host and untrusted code (skills, napps, agent
//! shell commands).
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │  Host (Nebo Desktop / CLI)                           │
//! │                                                      │
//! │  VmManager ──→ VmService (Swift/QEMU) ──→ VM Guest  │
//! │       │              │                        │      │
//! │       │         vsock / stdio            guest daemon │
//! │       │              │                        │      │
//! │  VmClient ←── RPC (len-prefixed JSON) ──→ RPC loop  │
//! │       │                                       │      │
//! │  spawn / kill / read_file / write_file   exec / fs   │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! ## Platforms
//!
//! - **macOS**: Apple Virtualization.framework via compiled Swift helper
//! - **Linux**: QEMU/KVM via direct process management
//! - **Windows**: Reserved for future Hyper-V support
//!
//! ## Wire Protocol
//!
//! Length-prefixed JSON over vsock (or stdio for dev):
//! ```text
//! [4 bytes: u32 big-endian length] [N bytes: UTF-8 JSON payload]
//! ```
//!
//! Max message size: 10 MB.

pub mod bundle;
pub mod error;
pub mod manager;
pub mod rpc;
pub mod session;
pub mod transfer;

#[cfg(target_os = "macos")]
pub mod platform_macos;

#[cfg(target_os = "linux")]
pub mod platform_linux;

pub use bundle::Bundle;
pub use error::{VmError, VmResult};
pub use manager::VmManager;
pub use rpc::VmClient;
pub use session::VmSession;
pub use transfer::FileTransfer;
