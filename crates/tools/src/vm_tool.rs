//! VM tool — opt-in isolated Linux environment for builds, toolchains, and
//! code execution that the host machine can't support natively.
//!
//! This is NOT a security sandbox. Nebo's existing safeguards (safeguard.rs,
//! sandbox-runtime, origin wall) handle security. The VM is a capability
//! extension: when the user needs Go, gcc, Docker, or a clean build environment
//! that their macOS/Windows host doesn't have.
//!
//! The agent explicitly invokes vm() — it is never implicit.
//!
//! ## Rootfs distribution
//!
//! The rootfs (Alpine + runtimes) is downloaded from CDN on first VM use,
//! matching Cowork's pattern:
//!   1. Check local rootfs.img with SHA origin file
//!   2. Decompress cached .zst if available
//!   3. Download from https://cdn.neboai.com/vm/{arch}/{sha}/rootfs.img.zst
//!
//! The sidecar image (nebo-vm.{arch}.img) ships embedded in the app bundle.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Current rootfs version SHA — updated when a new rootfs is published to CDN.
/// Set to empty string during development (skips CDN download, uses local build).
const ROOTFS_SHA: &str = "";

/// Tool that provides access to an isolated Linux VM for builds and toolchains.
pub struct VmTool {
    manager: Arc<RwLock<Option<vm::VmManager>>>,
}

impl VmTool {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(RwLock::new(None)),
        }
    }

    /// Ensure the VM is booted and return a reference to the manager.
    async fn ensure_running(&self) -> Result<(), String> {
        let mut guard = self.manager.write().await;
        if guard.is_none() {
            // Find the sidecar image (embedded in app bundle)
            let image_path = find_sidecar_image()
                .ok_or_else(|| "VM sidecar image not found. Run `make vm-image` to build.".to_string())?;

            // Resolve rootfs (download from CDN if needed)
            let rootfs_path = resolve_rootfs().await?;

            let config = vm::manager::VmConfig {
                image_path,
                rootfs_path,
                rootfs_sha: ROOTFS_SHA.to_string(),
                ..Default::default()
            };

            let manager = vm::VmManager::new(config);
            manager.start().await.map_err(|e| format!("failed to start VM: {e}"))?;
            *guard = Some(manager);
            info!("VM started for vm() tool");
        }
        Ok(())
    }

    async fn handle_exec(&self, input: &serde_json::Value) -> ToolResult {
        let command = match input.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error(crate::errors::missing_param(
                "exec",
                "command",
                "vm(action: \"exec\", command: \"ls -la /tmp\")",
            )),
        };

        let timeout = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        if let Err(e) = self.ensure_running().await {
            return ToolResult::error(e);
        }

        let guard = self.manager.read().await;
        let mgr = guard.as_ref().unwrap();

        // Create a session for this execution
        let session_id = match mgr.create_session("vm-exec", None).await {
            Ok(id) => id,
            Err(e) => return ToolResult::error(format!("failed to create session: {e}")),
        };

        // Execute
        let result = mgr
            .exec(&session_id, command, vec![], None, Some(timeout))
            .await;

        // Clean up session
        let _ = mgr.destroy_session(&session_id).await;

        match result {
            Ok((stdout, stderr, exit_code)) => {
                let mut output = String::new();
                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("[stderr]\n");
                    output.push_str(&stderr);
                }
                if exit_code != 0 {
                    output.push_str(&format!("\n[exit code: {}]", exit_code));
                }
                if output.is_empty() {
                    output = "(no output)".to_string();
                }
                if exit_code == 0 {
                    ToolResult::ok(output)
                } else {
                    ToolResult::error(output)
                }
            }
            Err(e) => ToolResult::error(format!("VM exec failed: {e}")),
        }
    }

    async fn handle_write_file(&self, input: &serde_json::Value) -> ToolResult {
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error(crate::errors::missing_param(
                "writeFile",
                "path",
                "vm(action: \"writeFile\", path: \"/tmp/hello.txt\", content: \"hello world\")",
            )),
        };
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::error(crate::errors::missing_param(
                "writeFile",
                "content",
                "vm(action: \"writeFile\", path: \"/tmp/hello.txt\", content: \"hello world\")",
            )),
        };

        if let Err(e) = self.ensure_running().await {
            return ToolResult::error(e);
        }

        let guard = self.manager.read().await;
        let mgr = guard.as_ref().unwrap();

        let params = vm::rpc::WriteFileParams {
            path: path.to_string(),
            content: content.to_string(),
            append: false,
        };
        match mgr.client().write_file(params).await {
            Ok(_) => ToolResult::ok(format!("wrote {} bytes to {path}", content.len())),
            Err(e) => ToolResult::error(format!("writeFile failed: {e}")),
        }
    }

    async fn handle_read_file(&self, input: &serde_json::Value) -> ToolResult {
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::error(crate::errors::missing_param(
                "readFile",
                "path",
                "vm(action: \"readFile\", path: \"/tmp/hello.txt\")",
            )),
        };

        if let Err(e) = self.ensure_running().await {
            return ToolResult::error(e);
        }

        let guard = self.manager.read().await;
        let mgr = guard.as_ref().unwrap();

        match vm::FileTransfer::read_from_vm(mgr.client(), path).await {
            Ok(content) => ToolResult::ok(content),
            Err(e) => ToolResult::error(format!("readFile failed: {e}")),
        }
    }

    async fn handle_copy_out(&self, input: &serde_json::Value) -> ToolResult {
        let vm_paths: Vec<String> = match input.get("paths") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => return ToolResult::error(crate::errors::missing_param(
                "copyOut",
                "paths",
                "vm(action: \"copyOut\", paths: [\"/tmp/output.txt\"], dest: \"~/Downloads/\")",
            )),
        };

        let dest = match input.get("dest").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return ToolResult::error(crate::errors::missing_param(
                "copyOut",
                "dest",
                "vm(action: \"copyOut\", paths: [\"/tmp/output.txt\"], dest: \"~/Downloads/\")",
            )),
        };

        if vm_paths.is_empty() {
            return ToolResult::error(
                "copyOut `paths` array is empty. Provide at least one VM file path to copy.\n\
                 Example: vm(action: \"copyOut\", paths: [\"/tmp/output.txt\"], dest: \"~/Downloads/\")",
            );
        }

        if let Err(e) = self.ensure_running().await {
            return ToolResult::error(e);
        }

        let guard = self.manager.read().await;
        let mgr = guard.as_ref().unwrap();

        match vm::FileTransfer::copy_to_host(mgr.client(), vm_paths, dest, false).await {
            Ok(result) => {
                let count = result.copied.len();
                ToolResult::ok(format!("copied {count} file(s) to {dest}"))
            }
            Err(e) => ToolResult::error(format!("copyOut failed: {e}")),
        }
    }

    async fn handle_list(&self, input: &serde_json::Value) -> ToolResult {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("/sessions");

        if let Err(e) = self.ensure_running().await {
            return ToolResult::error(e);
        }

        let guard = self.manager.read().await;
        let mgr = guard.as_ref().unwrap();

        match vm::FileTransfer::list_vm_dir(mgr.client(), path).await {
            Ok(entries) => {
                if entries.is_empty() {
                    ToolResult::ok(format!("{path}: (empty)"))
                } else {
                    ToolResult::ok(entries.join("\n"))
                }
            }
            Err(e) => ToolResult::error(format!("list failed: {e}")),
        }
    }

    async fn handle_status(&self) -> ToolResult {
        let guard = self.manager.read().await;
        match guard.as_ref() {
            Some(mgr) => {
                let state = mgr.state().await;
                ToolResult::ok(format!("VM state: {:?}", state))
            }
            None => ToolResult::ok("VM state: not started"),
        }
    }

    async fn handle_stop(&self) -> ToolResult {
        let mut guard = self.manager.write().await;
        if let Some(mgr) = guard.take() {
            match mgr.stop().await {
                Ok(_) => ToolResult::ok("VM stopped"),
                Err(e) => ToolResult::error(format!("failed to stop VM: {e}")),
            }
        } else {
            ToolResult::ok("VM was not running")
        }
    }
}

impl DynTool for VmTool {
    fn name(&self) -> &str {
        "vm"
    }

    fn description(&self) -> String {
        "Isolated Linux VM for builds and toolchains not available on the host.\n\n\
         Use this when the host machine lacks a required toolchain (Go, gcc, Docker, \
         specific Python/Node versions) or when a clean build environment is needed.\n\n\
         DO NOT use this for tasks that work fine on the host — shell, file ops, \
         desktop automation, browser, plugins, and skills all run on the host directly.\n\n\
         Resources and actions:\n\
         - exec: Run a shell command in the VM (e.g. `go build`, `gcc`, `make`)\n\
         - writeFile: Write content to a file inside the VM\n\
         - readFile: Read a file from the VM\n\
         - copyOut: Copy files from VM to a host directory\n\
         - list: List files in a VM directory\n\
         - status: Check VM state\n\
         - stop: Shut down the VM\n\n\
         The VM boots on first use. Rootfs is downloaded from CDN on first launch.\n\n\
         Examples:\n  \
         vm(action: \"exec\", command: \"go build -o myapp ./cmd/server\")\n  \
         vm(action: \"writeFile\", path: \"/sessions/work/main.go\", content: \"package main...\")\n  \
         vm(action: \"copyOut\", paths: [\"/sessions/work/myapp\"], dest: \"~/projects/myapp/\")\n  \
         vm(action: \"exec\", command: \"python3 -m pytest\", timeout: 300)"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["exec", "writeFile", "readFile", "copyOut", "list", "status", "stop"],
                    "description": "REQUIRED. The VM operation to perform."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run (for action: exec)."
                },
                "path": {
                    "type": "string",
                    "description": "File path inside the VM (for readFile, writeFile, list)."
                },
                "content": {
                    "type": "string",
                    "description": "File content to write (for action: writeFile)."
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "VM paths to copy out (for action: copyOut)."
                },
                "dest": {
                    "type": "string",
                    "description": "Host destination directory (for action: copyOut)."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds for exec (default: 120)."
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = match input.get("action").and_then(|v| v.as_str()) {
                Some(a) => a.to_string(),
                None => return ToolResult::error(crate::errors::missing_param(
                    "vm",
                    "action",
                    "vm(action: \"exec\", command: \"ls -la\")\nAvailable actions: exec, writeFile, readFile, copyOut, list, status, stop",
                )),
            };

            match action.as_str() {
                "exec" => self.handle_exec(&input).await,
                "writeFile" => self.handle_write_file(&input).await,
                "readFile" => self.handle_read_file(&input).await,
                "copyOut" => self.handle_copy_out(&input).await,
                "list" => self.handle_list(&input).await,
                "status" => self.handle_status().await,
                "stop" => self.handle_stop().await,
                other => ToolResult::error(format!(
                    "Unknown vm action: `{other}`. Use: exec, writeFile, readFile, copyOut, list, status, stop"
                )),
            }
        })
    }
}

/// Resolve the rootfs image path. Uses the Bundle system to download from CDN
/// if not available locally.
async fn resolve_rootfs() -> Result<String, String> {
    // Development: check for local build first
    let dev_rootfs = "vm/build/rootfs.img";
    if std::path::Path::new(dev_rootfs).exists() {
        info!("using dev rootfs: {dev_rootfs}");
        return Ok(dev_rootfs.to_string());
    }

    // Production: use bundle manager (CDN download + cache + SHA verify)
    if ROOTFS_SHA.is_empty() {
        // No SHA configured — check for any local rootfs in bundle dir
        if let Some(home) = dirs::home_dir() {
            let bundle_rootfs = home.join(".nebo").join("vm").join("bundles").join("rootfs.img");
            if bundle_rootfs.exists() {
                return Ok(bundle_rootfs.to_string_lossy().to_string());
            }
        }
        return Err(
            "No rootfs available. Run `make vm-rootfs` for dev, or set ROOTFS_SHA for CDN download."
                .to_string(),
        );
    }

    let bundle = vm::Bundle::new(ROOTFS_SHA)
        .map_err(|e| format!("bundle init failed: {e}"))?;

    let rootfs_path = bundle
        .ensure_rootfs()
        .await
        .map_err(|e| format!("rootfs download failed: {e}"))?;

    bundle.clear_reinstall_marker();

    Ok(rootfs_path.to_string_lossy().to_string())
}

/// Find the sidecar image (nebo-vm.{arch}.img) on disk. Checks:
/// 1. vm/build/ directory (development builds)
/// 2. ~/.nebo/vm/ (installed images)
/// 3. Bundled with app (Tauri resource)
fn find_sidecar_image() -> Option<String> {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };

    // Development: check vm/build/ relative to project
    let dev_path = format!("vm/build/nebo-vm.{arch}.img");
    if std::path::Path::new(&dev_path).exists() {
        return Some(dev_path);
    }

    // Installed: ~/.nebo/vm/
    if let Some(home) = dirs::home_dir() {
        let installed = home.join(".nebo").join("vm").join(format!("nebo-vm.{arch}.img"));
        if installed.exists() {
            return Some(installed.to_string_lossy().to_string());
        }
    }

    // Tauri resource: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(resources) = exe.parent().and_then(|p| p.parent()) {
            // macOS: Contents/Resources/
            let resource_path = resources.join("Resources").join(format!("nebo-vm.{arch}.img"));
            if resource_path.exists() {
                return Some(resource_path.to_string_lossy().to_string());
            }
        }
    }

    None
}
