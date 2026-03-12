//! Execute tool — runs scripts bundled with skills.
//!
//! Three execution paths:
//! 1. Local subprocess with OS-level sandbox (macOS Seatbelt / Linux bubblewrap)
//! 2. Cloud sandbox via Janus POST /v1/execute (paid tier)
//! 3. Structured error with upgrade/install options

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sandbox_runtime::SandboxManager;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::skills::Loader;

/// How a script runtime was resolved.
#[derive(Debug)]
enum RuntimeKind {
    /// Bundled `uv` in /tmp/nebo-runtimes/ — runs as `uv run script.py`
    Uv(PathBuf),
    /// Bundled `bun` in /tmp/nebo-runtimes/ — runs as `bun run script.ts`
    Bun(PathBuf),
    /// System-installed runtime found via `which`
    System(PathBuf),
    /// Pre-compiled binary extracted from .napp — runs directly
    Binary,
}

/// Tool that executes scripts bundled with skills.
pub struct ExecuteTool {
    loader: Arc<Loader>,
    plan_tier: Arc<RwLock<String>>,
    sandbox: Option<Arc<SandboxManager>>,
}

impl ExecuteTool {
    pub fn new(
        loader: Arc<Loader>,
        plan_tier: Arc<RwLock<String>>,
        sandbox: Option<Arc<SandboxManager>>,
    ) -> Self {
        Self {
            loader,
            plan_tier,
            sandbox,
        }
    }

    /// Detect language from file extension.
    fn detect_language(script_path: &str) -> Option<&'static str> {
        if script_path.ends_with(".py") {
            Some("python")
        } else if script_path.ends_with(".ts") {
            Some("typescript")
        } else if script_path.ends_with(".js") {
            Some("javascript")
        } else {
            None
        }
    }

    /// Resolve runtime: bundled /tmp/nebo-runtimes/ first, then system PATH.
    fn find_runtime(language: &str) -> Option<RuntimeKind> {
        let runtimes_dir = Path::new("/tmp/nebo-runtimes");

        match language {
            "python" => {
                // Check bundled uv first
                let uv_path = runtimes_dir.join("uv");
                if uv_path.is_file() {
                    return Some(RuntimeKind::Uv(uv_path));
                }
                // Fall back to system python
                which::which("python3")
                    .or_else(|_| which::which("python"))
                    .ok()
                    .map(RuntimeKind::System)
            }
            "typescript" | "javascript" => {
                // Check bundled bun first
                let bun_path = runtimes_dir.join("bun");
                if bun_path.is_file() {
                    return Some(RuntimeKind::Bun(bun_path));
                }
                // Fall back to system node
                which::which("node").ok().map(RuntimeKind::System)
            }
            _ => None,
        }
    }

    /// Extract ALL skill resources into a temp directory for multi-file support.
    fn extract_resources(
        skill: &crate::skills::Skill,
        tmp_dir: &Path,
    ) -> Result<(), String> {
        let resources = skill.list_resources()?;
        for rel_path in &resources {
            let data = skill.read_resource(rel_path)?;
            let dest = tmp_dir.join(rel_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create dir {}: {}", parent.display(), e))?;
            }
            std::fs::write(&dest, &data)
                .map_err(|e| format!("failed to write {}: {}", dest.display(), e))?;
        }
        Ok(())
    }

    /// Build the command string for the resolved runtime.
    fn build_command(runtime: &RuntimeKind, language: &str, script_path: &Path) -> String {
        let script = script_path.to_string_lossy();
        match runtime {
            RuntimeKind::Uv(uv) => {
                format!("{} run {}", uv.to_string_lossy(), script)
            }
            RuntimeKind::Bun(bun) => {
                format!("{} run {}", bun.to_string_lossy(), script)
            }
            RuntimeKind::System(bin) => {
                // For TypeScript with system node, try tsx first
                if language == "typescript" {
                    if let Ok(tsx) = which::which("tsx") {
                        return format!("{} {}", tsx.to_string_lossy(), script);
                    }
                }
                format!("{} {}", bin.to_string_lossy(), script)
            }
            RuntimeKind::Binary => {
                // Binary runs directly — script_path IS the executable
                script.to_string()
            }
        }
    }

    /// Execute a script locally, optionally wrapped in an OS sandbox.
    async fn execute_local(
        &self,
        runtime: &RuntimeKind,
        language: &str,
        skill: &crate::skills::Skill,
        script_rel_path: &str,
        args: &serde_json::Value,
        timeout_secs: u64,
    ) -> ToolResult {
        // Create temp dir and extract all skill resources into it
        let tmp_dir = match tempfile::TempDir::new() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("failed to create temp dir: {}", e)),
        };

        if let Err(e) = Self::extract_resources(skill, tmp_dir.path()) {
            return ToolResult::error(format!("failed to extract skill resources: {}", e));
        }

        let script_path = tmp_dir.path().join(script_rel_path);
        if !script_path.exists() {
            return ToolResult::error(format!(
                "Script '{}' not found after extracting resources",
                script_rel_path
            ));
        }

        // Build the base command string
        let base_cmd = Self::build_command(runtime, language, &script_path);

        // Try to wrap with sandbox if available
        let final_cmd = if let Some(ref sandbox) = self.sandbox {
            let sandbox_config =
                crate::sandbox_policy::build_sandbox_config(skill, tmp_dir.path());
            match sandbox
                .wrap_with_sandbox_opts(&base_cmd, None, Some(&sandbox_config))
                .await
            {
                Ok(wrapped) => {
                    debug!("executing with sandbox: {}", &wrapped[..wrapped.len().min(120)]);
                    wrapped
                }
                Err(e) => {
                    warn!("sandbox wrap failed, falling back to bare execution: {}", e);
                    base_cmd
                }
            }
        } else {
            base_cmd
        };

        // Execute via sh -c (or powershell on Windows)
        let (shell, shell_args) = crate::process::shell_command();
        let mut cmd = tokio::process::Command::new(&shell);
        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(&final_cmd);

        // Pass args as environment variable
        if !args.is_null() {
            cmd.env("SKILL_ARGS", args.to_string());
        }

        crate::process::hide_window(&mut cmd);
        cmd.current_dir(tmp_dir.path());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        debug!(language, cmd = %final_cmd, "executing script locally");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            cmd.output(),
        )
        .await;

        // Post-execution sandbox cleanup
        if let Some(ref sandbox) = self.sandbox {
            sandbox.cleanup_after_command();
        }

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut stderr_str = String::from_utf8_lossy(&output.stderr).to_string();

                // Annotate sandbox violations in stderr if sandbox is active
                if let Some(ref sandbox) = self.sandbox {
                    stderr_str =
                        sandbox.annotate_stderr_with_sandbox_failures(&stderr_str, &final_cmd);
                }

                let exit_code = output.status.code().unwrap_or(-1);

                if output.status.success() {
                    let mut result = stdout.to_string();
                    if !stderr_str.is_empty() {
                        result.push_str(&format!("\n[stderr]\n{}", stderr_str));
                    }
                    ToolResult::ok(result)
                } else {
                    ToolResult::error(format!(
                        "Script exited with code {}\n[stdout]\n{}\n[stderr]\n{}",
                        exit_code, stdout, stderr_str
                    ))
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("failed to execute script: {}", e)),
            Err(_) => ToolResult::error(format!(
                "Script timed out after {} seconds",
                timeout_secs
            )),
        }
    }
}

impl DynTool for ExecuteTool {
    fn name(&self) -> &str {
        "execute"
    }

    fn description(&self) -> String {
        "Execute a script or binary bundled with a skill. Runs locally if the runtime is installed, \
         or in the cloud sandbox for paid tiers.\n\n\
         Examples:\n  \
         execute(skill: \"xlsx-processor\", script: \"scripts/recalc.py\", args: {\"file\": \"output.xlsx\"})\n  \
         execute(skill: \"pptx\", script: \"binary\", args: {\"command\": \"create\", \"spec\": {...}})"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill containing the script"
                },
                "script": {
                    "type": "string",
                    "description": "Relative path to the script within the skill (e.g. 'scripts/recalc.py'), or 'binary' to run the pre-compiled executable"
                },
                "args": {
                    "type": "object",
                    "description": "Arguments to pass to the script as SKILL_ARGS env var"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Execution timeout in seconds (default: 30)",
                    "default": 30
                }
            },
            "required": ["skill", "script"]
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
            let skill_name = match input["skill"].as_str() {
                Some(s) if !s.is_empty() => s,
                _ => return ToolResult::error("skill is required"),
            };
            let script_path = match input["script"].as_str() {
                Some(s) if !s.is_empty() => s,
                _ => return ToolResult::error("script is required"),
            };
            let args = input.get("args").cloned().unwrap_or(serde_json::json!(null));
            let timeout = input["timeout"].as_u64().unwrap_or(30);

            // 1. Look up skill
            let skill = match self.loader.get(skill_name).await {
                Some(s) => s,
                None => return ToolResult::error(format!("Skill '{}' not found", skill_name)),
            };

            // 2. Check for binary execution — bin/ directory or legacy root "binary"
            let is_bin = script_path == "binary"
                || script_path.starts_with("bin/");
            if is_bin {
                if let Some(ref base_dir) = skill.base_dir {
                    // Try exact path first (e.g. "bin/nebo-office"), then legacy "binary"
                    let binary_path = base_dir.join(script_path);
                    if binary_path.is_file() {
                        debug!(skill = skill_name, path = %binary_path.display(), "executing binary from .napp");
                        return self
                            .execute_local(&RuntimeKind::Binary, "binary", &skill, script_path, &args, timeout)
                            .await;
                    }
                    // Legacy: root-level "binary" entry
                    if script_path == "binary" {
                        let legacy = base_dir.join("binary");
                        if legacy.is_file() {
                            debug!(skill = skill_name, "executing legacy binary from .napp");
                            return self
                                .execute_local(&RuntimeKind::Binary, "binary", &skill, "binary", &args, timeout)
                                .await;
                        }
                    }
                }
                return ToolResult::error(format!(
                    "Skill '{}' has no binary at '{}'. The .napp archive may not contain a binary for this platform.",
                    skill_name, script_path
                ));
            }

            // 3. Detect language
            let language = match Self::detect_language(script_path) {
                Some(lang) => lang,
                None => {
                    return ToolResult::error(format!(
                        "Unsupported script type: {}. Supported: .py, .ts, .js, or 'binary'",
                        script_path
                    ))
                }
            };

            // 4. Try local execution first
            if let Some(runtime) = Self::find_runtime(language) {
                debug!(language, ?runtime, "found runtime");
                return self
                    .execute_local(&runtime, language, &skill, script_path, &args, timeout)
                    .await;
            }

            // 5. Try cloud sandbox (Janus)
            let tier = self.plan_tier.read().await.clone();
            if tier == "pro" || tier == "team" || tier == "enterprise" {
                // TODO: Wire to POST {janus_url}/v1/execute when Janus endpoint is ready.
                // For now, return a helpful message.
                warn!(skill = skill_name, script = script_path, "cloud sandbox not yet available");
                return ToolResult::error(
                    "Cloud sandbox execution is coming soon. \
                     Install the runtime locally to run this script now:\n\
                     - Python: https://python.org/downloads/\n\
                     - Node.js: https://nodejs.org/",
                );
            }

            // 6. Neither available — show both options
            ToolResult::error(format!(
                "No {} runtime found locally and cloud sandbox requires a paid plan.\n\n\
                 Option 1: Install {} locally (free)\n\
                 {}\n\n\
                 Option 2: Upgrade to Pro for cloud execution\n\
                 Visit your NeboLoop dashboard to upgrade.",
                language,
                language,
                match language {
                    "python" => "  https://python.org/downloads/",
                    _ => "  https://nodejs.org/",
                }
            ))
        })
    }
}
