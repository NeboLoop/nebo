//! Linux VM backend using QEMU/KVM.
//!
//! On Linux, we use QEMU with KVM acceleration to run the guest VM.
//! The guest daemon communicates via stdio (piped through QEMU's serial port).

use crate::error::{VmError, VmResult};
use crate::manager::VmConfig;
use tracing::info;

/// Start a VM using QEMU with KVM acceleration.
///
/// Returns the child process whose stdio is piped for RPC.
pub async fn start_vm(
    config: &VmConfig,
) -> VmResult<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("qemu-system-x86_64");

    cmd.args([
        "-enable-kvm",
        "-m", &format!("{}M", config.memory_mb),
        "-smp", &config.cpu_count.to_string(),
        "-drive", &format!("file={},format=raw,if=virtio", config.image_path),
        // Serial port on stdio for guest daemon RPC
        "-serial", "stdio",
        // No graphics
        "-nographic",
        // NAT networking
        "-netdev", "user,id=net0",
        "-device", "virtio-net-pci,netdev=net0",
    ]);

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let child = cmd.spawn()?;
    info!("QEMU VM process started");
    Ok(child)
}

/// Check if KVM is available.
pub fn is_supported() -> bool {
    std::path::Path::new("/dev/kvm").exists()
}

/// Check system requirements for QEMU/KVM.
pub async fn check_requirements() -> Vec<String> {
    let mut issues = Vec::new();

    // Check QEMU availability
    match tokio::process::Command::new("qemu-system-x86_64")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {}
        _ => issues.push("qemu-system-x86_64 not found — install qemu".to_string()),
    }

    // Check KVM support
    if !std::path::Path::new("/dev/kvm").exists() {
        issues.push("/dev/kvm not found — KVM acceleration not available".to_string());
    }

    issues
}
