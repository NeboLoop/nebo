//! File transfer between VM and host.
//!
//! Security model: The host ALWAYS pulls from the VM. The VM cannot push files
//! to the host filesystem directly. This ensures that even if code running in
//! the VM is compromised, it cannot write to sensitive host paths like
//! ~/.bashrc, ~/.ssh/, etc.
//!
//! Transfer flow:
//! ```text
//! Agent: "save project files to ~/projects/myapp"
//!   → Host validates destination path (safeguards apply)
//!   → Host sends copyOut RPC to guest with source paths
//!   → Guest reads files, base64-encodes, sends back via RPC response
//!   → Host decodes and writes to approved destination
//! ```

use crate::error::VmResult;
use crate::rpc::{CopyOutParams, CopyOutResult, VmClient};
use tracing::info;

/// File transfer manager for VM-to-host operations.
pub struct FileTransfer;

impl FileTransfer {
    /// Copy files from the VM to a host directory.
    ///
    /// # Security
    ///
    /// The `dest_dir` must be validated by the caller before calling this.
    /// This function does NOT check safeguards — that's the tool layer's job.
    ///
    /// # Arguments
    ///
    /// * `client` - Connected VM RPC client
    /// * `vm_paths` - Paths inside the VM to copy out
    /// * `dest_dir` - Host directory to write files into
    /// * `flatten` - If true, strip directory structure and write all files flat
    pub async fn copy_to_host(
        client: &VmClient,
        vm_paths: Vec<String>,
        dest_dir: &str,
        _flatten: bool,
    ) -> VmResult<CopyOutResult> {
        // Ensure destination exists
        std::fs::create_dir_all(dest_dir)?;

        let params = CopyOutParams {
            src_paths: vm_paths,
            dest_dir: dest_dir.to_string(),
        };

        info!(
            dest = %dest_dir,
            count = params.src_paths.len(),
            "copying files from VM to host"
        );

        client.copy_out(params).await
    }

    /// Copy a single file from the VM and return its content as a string.
    ///
    /// Useful for reading build outputs, logs, or generated code.
    pub async fn read_from_vm(client: &VmClient, vm_path: &str) -> VmResult<String> {
        client.read_file(vm_path).await
    }

    /// List files in a VM directory (for browsing before copy-out).
    pub async fn list_vm_dir(client: &VmClient, vm_path: &str) -> VmResult<Vec<String>> {
        let result = client
            .request(
                "listDir",
                Some(serde_json::json!({ "path": vm_path })),
            )
            .await?;

        let entries: Vec<String> = result
            .get("entries")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(entries)
    }
}
