//! Plugin-provided AI providers.
//!
//! Plugins can register as AI providers (e.g., OpenRouter, local model servers).
//! Communication uses NDJSON on stdout: the plugin binary receives a ChatRequest
//! on stdin and writes streaming events as JSON lines.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::warn;

use ai::{ChatRequest, EventReceiver, Provider, ProviderError, StreamEvent, StreamEventType, ToolCall, UsageInfo};

/// A provider backed by a plugin binary using NDJSON streaming.
pub struct PluginProvider {
    provider_id: String,
    display_name: String,
    binary_path: PathBuf,
    chat_command: Vec<String>,
    plugin_slug: String,
    plugin_store: Arc<napp::plugin::PluginStore>,
}

impl PluginProvider {
    pub fn new(
        def: &napp::plugin::PluginProviderDef,
        plugin_slug: &str,
        binary_path: PathBuf,
        plugin_store: Arc<napp::plugin::PluginStore>,
    ) -> Self {
        let chat_command: Vec<String> = def.chat_command.split_whitespace().map(String::from).collect();

        Self {
            provider_id: def.id.clone(),
            display_name: def.display_name.clone(),
            binary_path,
            chat_command,
            plugin_slug: plugin_slug.to_string(),
            plugin_store,
        }
    }
}

#[async_trait]
impl Provider for PluginProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        let (tx, rx) = mpsc::channel(64);

        let mut cmd = tokio::process::Command::new(&self.binary_path);
        cmd.args(&self.chat_command);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Augmented PATH so plugin can find dep binaries
        cmd.env("PATH", self.plugin_store.path_with_plugins());

        // Auth env vars
        if let Some((_bin, auth)) = self.plugin_store.get_auth_info(&self.plugin_slug) {
            for (k, v) in &auth.env {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::Request(format!(
                "plugin provider '{}' spawn failed: {}",
                self.provider_id, e
            ))
        })?;

        // Write ChatRequest JSON to stdin
        let req_json = serde_json::to_vec(req).map_err(|e| {
            ProviderError::Request(format!("serialize request: {}", e))
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&req_json).await;
            drop(stdin); // close stdin so plugin knows input is complete
        }

        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::Request("no stdout from plugin provider".into())
        })?;

        let provider_id = self.provider_id.clone();

        // Spawn reader task: parse NDJSON lines from stdout into StreamEvents
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_ndjson_event(&line) {
                    Some(event) => {
                        if tx.send(event).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                    None => {
                        warn!(provider = %provider_id, line = %line, "unparseable NDJSON line");
                    }
                }
            }

            // Send done event
            let _ = tx.send(StreamEvent::done()).await;

            // Wait for child to finish
            let _ = child.wait().await;
        });

        Ok(rx)
    }
}

/// Parse a single NDJSON line into a StreamEvent.
///
/// Expected format:
/// - `{"type":"text","content":"Hello"}` → StreamEvent::Text
/// - `{"type":"tool_call","id":"tc1","name":"web","input":{}}` → StreamEvent::ToolCall
/// - `{"type":"done","usage":{"input_tokens":10,"output_tokens":20}}` → StreamEvent::Done
/// - `{"type":"error","content":"something went wrong"}` → StreamEvent::Error
fn parse_ndjson_event(line: &str) -> Option<StreamEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let event_type = v.get("type")?.as_str()?;

    match event_type {
        "text" => {
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            Some(StreamEvent::text(content))
        }
        "tool_call" => {
            let mut ev = StreamEvent::text("");
            ev.event_type = StreamEventType::ToolCall;
            ev.text = String::new();
            ev.tool_call = Some(ToolCall {
                id: v.get("id").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                name: v.get("name").and_then(|c| c.as_str()).unwrap_or("").to_string(),
                input: v.get("input").cloned().unwrap_or(serde_json::Value::Object(Default::default())),
            });
            Some(ev)
        }
        "done" => {
            let mut ev = StreamEvent::done();
            ev.usage = v.get("usage").and_then(|u| {
                Some(UsageInfo {
                    input_tokens: u.get("input_tokens").and_then(|t| t.as_i64()).unwrap_or(0) as i32,
                    output_tokens: u.get("output_tokens").and_then(|t| t.as_i64()).unwrap_or(0) as i32,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                })
            });
            Some(ev)
        }
        "error" => {
            let msg = v.get("content").and_then(|c| c.as_str()).unwrap_or("unknown error");
            Some(StreamEvent::error(msg))
        }
        _ => None,
    }
}
