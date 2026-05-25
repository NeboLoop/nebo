//! macOS VM backend using Apple Virtualization.framework.
//!
//! Follows the same pattern as the PIM helper in crates/tools/src/organizer/native.rs:
//! - Swift source embedded at compile time via include_str!()
//! - Compiled on first use via swiftc, cached with hash-based invalidation
//! - Invoked as a subprocess with CLI arguments
//!
//! The Swift helper manages the VM lifecycle:
//! - create: Create a VM configuration and disk image
//! - start: Boot the VM, establish stdio pipe for guest daemon RPC
//! - stop: Graceful shutdown
//! - status: Check if running

use crate::error::{VmError, VmResult};
use crate::manager::VmConfig;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tracing::info;

/// Cached path to the compiled VM helper binary.
static HELPER_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// Swift source for the Virtualization.framework wrapper.
const VM_HELPER_SOURCE: &str = include_str!("vm_helper.swift");

/// FNV-1a hash for recompile detection.
const SOURCE_HASH: u64 = const_fnv1a(VM_HELPER_SOURCE.as_bytes());

const fn const_fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Ensure the VM helper binary is compiled and cached.
async fn ensure_helper() -> VmResult<PathBuf> {
    let mutex = HELPER_PATH.get_or_init(|| Mutex::new(None));
    let mut guard = mutex.lock().await;

    if let Some(ref path) = *guard {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    let bin_dir = config::data_dir()
        .map_err(|e| VmError::Io(std::io::Error::other(format!("no data dir: {e}"))))?
        .join("bin");
    std::fs::create_dir_all(&bin_dir)?;

    let binary_path = bin_dir.join("nebo-vm-helper");
    let hash_path = bin_dir.join("nebo-vm-helper.hash");

    // Check if existing binary matches current source
    if binary_path.exists() {
        if let Ok(stored) = std::fs::read_to_string(&hash_path) {
            if stored.trim() == SOURCE_HASH.to_string() {
                *guard = Some(binary_path.clone());
                return Ok(binary_path);
            }
            info!("VM helper source changed, recompiling…");
        }
        let _ = std::fs::remove_file(&binary_path);
    }

    // Write source and compile
    let source_path = bin_dir.join("vm_helper.swift");
    std::fs::write(&source_path, VM_HELPER_SOURCE)?;

    info!("compiling native VM helper (Virtualization.framework)…");
    let output = tokio::process::Command::new("swiftc")
        .args(["-O", "-framework", "Virtualization"])
        .arg("-o")
        .arg(&binary_path)
        .arg(&source_path)
        .output()
        .await?;

    if output.status.success() {
        info!(path = %binary_path.display(), "VM helper compiled");
        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::write(&hash_path, SOURCE_HASH.to_string());
        *guard = Some(binary_path.clone());
        Ok(binary_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&source_path);
        Err(VmError::SwiftCompilationFailed(stderr.to_string()))
    }
}

/// Start a VM using Virtualization.framework.
///
/// Returns the child process whose stdio is piped for RPC.
pub async fn start_vm(
    config: &VmConfig,
) -> VmResult<tokio::process::Child> {
    let helper = ensure_helper().await?;

    let mut cmd = tokio::process::Command::new(&helper);
    cmd.arg("start")
        .arg("--memory-mb")
        .arg(config.memory_mb.to_string())
        .arg("--cpus")
        .arg(config.cpu_count.to_string())
        .arg("--image")
        .arg(&config.image_path);

    // The VM helper's stdio becomes the RPC channel to the guest daemon.
    // The Swift helper bridges: host stdio <-> VM vsock <-> guest daemon stdio
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let child = cmd.spawn()?;
    info!("VM helper process started");
    Ok(child)
}

/// Stop a VM by sending SIGTERM to the helper process.
pub async fn stop_vm(helper_pid: u32) -> VmResult<()> {
    let _ = tokio::process::Command::new("kill")
        .arg("-TERM")
        .arg(helper_pid.to_string())
        .output()
        .await;
    Ok(())
}

/// Check if the VM helper supports our platform.
pub fn is_supported() -> bool {
    cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")
}

/// Check system requirements.
pub async fn check_requirements() -> Vec<String> {
    let mut issues = Vec::new();

    // Check swiftc availability
    match tokio::process::Command::new("swiftc")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {}
        _ => issues.push("swiftc not found — install Xcode or Command Line Tools".to_string()),
    }

    // Check macOS version (need 13+ for Virtualization.framework Linux VMs)
    match tokio::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let major: u32 = version
                .split('.')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if major < 13 {
                issues.push(format!(
                    "macOS {version} detected — macOS 13+ required for Linux VMs"
                ));
            }
        }
        _ => issues.push("could not determine macOS version".to_string()),
    }

    issues
}
