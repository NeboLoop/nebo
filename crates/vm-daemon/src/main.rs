//! Nebo VM Sandbox Daemon
//!
//! Runs inside the lightweight Linux VM and handles:
//! - Process spawning with per-session isolation
//! - File operations (read, write, list, copy-out)
//! - Network proxy management
//! - Stdout/stderr event streaming back to host
//!
//! Communication: Length-prefixed JSON over stdio (vsock will be added later).
//!
//! Compiled as a static musl binary for minimal VM image size:
//!   cargo build --target aarch64-unknown-linux-musl --release -p nebo-vm-daemon

mod handler;
mod process;
mod wire;

use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nebo_vm_daemon=info".parse().unwrap()),
        )
        .with_target(false)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "nebo-vm-daemon daemon starting"
    );

    // The guest communicates with the host via stdio (stdin/stdout).
    // In production this will be vsock, but stdio works for dev/testing
    // and the wire protocol is the same.
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    if let Err(e) = handler::run(stdin, stdout).await {
        error!(%e, "guest daemon exited with error");
        std::process::exit(1);
    }
}
