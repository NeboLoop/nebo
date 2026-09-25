use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use db::Store;

/// Broadcast callback injected by the server (wired to ClientHub). Lets tools
/// surface owner notifications to the frontend (bell + desktop HUD) without
/// crates/tools depending on the server's hub — the same boundary-clean
/// pattern the agent worker uses (`agent::agent_worker::NotifyFn`).
pub type NotifyFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// MessageTool sends and reads SMS. Coworkers and teams are `send_message`;
/// reaching the owner is `message_owner`, `push_notification` and
/// `check_dnd` (`owner_tools`).
pub struct MessageTool {
    store: Arc<Store>,
}

impl MessageTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl DynTool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> String {
        "SMS — send, list, read and search text messages with people outside NeboAI.\n\
         To message a coworker (another employee on this Nebo) or a team, use send_message.\n\n\
         - message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\") — Send SMS (macOS)\n\
         - message(resource: \"sms\", action: \"conversations\") — List SMS conversations\n\
         - message(resource: \"sms\", action: \"read\", phone: \"+15551234567\") — Read SMS messages\n\
         - message(resource: \"sms\", action: \"search\", query: \"meeting\") — Search SMS messages\n\n\
         For text-to-speech: use os(resource: \"tts\", action: \"speak\", text: \"Hello\")\n\
         Use message for outbound delivery to humans outside NeboAI."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "REQUIRED. The messaging resource category — determines which actions are available.",
                    "enum": ["sms"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["send", "conversations", "read", "search"]
                },
                "text": { "type": "string", "description": "Message text" },
                "phone": { "type": "string", "description": "Phone number or contact for SMS" },
                "from": { "type": "string", "description": "SMS send: which of your phone lines to text from (E.164). Omit to use your first texting line." },
                "query": { "type": "string", "description": "Search query for SMS search" },
                "limit": { "type": "integer", "description": "Max number of results to return", "default": 20 }
            },
            "required": ["resource", "action"]
        })
    }


    fn search_hint(&self) -> &str {
        "send read search sms text messages"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let resource = input
            .get("resource")
            .and_then(|v| v.as_str())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| infer_resource(action));
        match (resource, action) {
            ("sms", "send") => "sms_message_send",
            ("sms", "conversations") => "sms_conversations",
            ("sms", "search") => "sms_search",
            ("sms", _) => "sms_read",
            _ => "message",
        }
        .to_string()
    }

    /// Who a text goes to: the phone number a recipient rule matches.
    fn rule_field(&self, input: &serde_json::Value) -> Option<types::permissions::RuleField> {
        let to = input.get("phone").and_then(|v| v.as_str()).filter(|t| !t.trim().is_empty())?;
        Some(types::permissions::RuleField::Recipient(to.trim().to_string()))
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        matches!(self.rule_key(input).as_str(), "sms_conversations" | "sms_search" | "sms_read")
    }

    /// SMS reads bring outside people's words into the run.
    fn taint(&self, input: &serde_json::Value) -> Option<types::provenance::ProvenanceClass> {
        matches!(self.rule_key(input).as_str(), "sms_conversations" | "sms_search" | "sms_read")
            .then_some(types::provenance::ProvenanceClass::Channel)
    }

    /// Pre-interface: it settles its own call shapes (see
    /// `DynTool::validates_input`).
    fn validates_input(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            // Attribute sends to the calling AI employee.
            let agent_id =
                Some(types::keyparser::extract_agent_id(&ctx.session_key)).filter(|s| !s.is_empty());
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Input did not match the schema: {}. Every call needs resource (sms) and action; fix the call and send it again.", e)),
            };

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["sms"],
                );
                if corrected.is_empty() {
                    infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            match resource.as_str() {
                "sms" => handle_sms(&self.store, ctx, agent_id.as_deref(), &domain_input.action, &input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: sms. To message a coworker or a team, use send_message.",
                    other
                )),
            }
        })
    }
}

/// Every action of this tool is an SMS action.
fn infer_resource(action: &str) -> &'static str {
    match action {
        "send" | "conversations" | "read" | "search" => "sms",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// SMS resource handlers (macOS Messages.app via chat.db)
// ---------------------------------------------------------------------------

async fn handle_sms(store: &Store, ctx: &ToolContext, agent_id: Option<&str>, action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        // A text to a customer goes through the effect ledger: recorded
        // before it goes, never sent twice for the same input in one run,
        // held for the owner when the outcome is unknown.
        "send" => {
            // Bad input never reaches the ledger: nothing was going to leave.
            let text = input["text"].as_str().unwrap_or("");
            let phone = input["phone"].as_str().unwrap_or("");
            if text.is_empty() {
                return ToolResult::error(errors::missing_param("send", "text", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
            }
            if phone.is_empty() {
                return ToolResult::error(errors::missing_param("send", "phone", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
            }
            crate::effects::guarded_send(store, ctx, "messaging", "sms", "sms.message.send", input, || async {
                match send_from_phone_line(store, agent_id, input).await {
                    Some(outcome) => outcome,
                    // No texting line: the owner's Messages.app. AppleScript
                    // raises before anything is dispatched, so an error is a
                    // confirmed failure; a clean hand-off is the send.
                    None => {
                        let r = handle_sms_send(input).await;
                        if r.is_error {
                            crate::effects::SendOutcome::ConfirmedFailure(r.content)
                        } else {
                            crate::effects::SendOutcome::Sent(r.content, None)
                        }
                    }
                }
            })
            .await
        }
        "conversations" => handle_sms_conversations(input).await,
        "read" => handle_sms_read(input).await,
        "search" => handle_sms_search(input).await,
        other => ToolResult::error(format!(
            "Unknown action '{}' for sms resource. Available: send, conversations, read, search",
            other
        )),
    }
}

/// An employee with a texting-enabled phone line texts from that line — the
/// business number the caller already knows — through the hub. `None` means
/// this employee has no such line, and the send falls through to the
/// owner's Messages.app (the pre-existing personal-device path).
/// Typed by the hub client's error: a refusal is a confirmed failure, no
/// answer is unknown, acceptance is the send with its reference.
async fn send_from_phone_line(store: &Store, agent_id: Option<&str>, input: &serde_json::Value) -> Option<crate::effects::SendOutcome> {
    use crate::effects::SendOutcome;
    let agent_id = agent_id?;
    let text = input["text"].as_str().unwrap_or("");
    let phone = input["phone"].as_str().unwrap_or("");
    let api = crate::build_neboai_api(store).ok()?;
    let lines = api.list_phone_lines().await.ok()?;
    let wanted = input["from"].as_str().filter(|s| !s.is_empty());
    let mine: Vec<&serde_json::Value> = lines["numbers"]
        .as_array()?
        .iter()
        .filter(|l| l["agentId"].as_str() == Some(agent_id) && l["smsEnabled"].as_bool() == Some(true))
        .collect();
    let line = match wanted {
        Some(w) => mine.iter().copied().find(|l| l["number"].as_str() == Some(w)),
        None => mine.first().copied(),
    };
    if wanted.is_some() && line.is_none() {
        return Some(SendOutcome::PreSendFailure(format!(
            "{} is not one of your texting lines. Omit `from` to use your first texting line.",
            wanted.unwrap_or("")
        )));
    }
    let line = line?;
    let from = line["number"].as_str()?.to_string();
    Some(match api.send_phone_sms(&from, phone, text).await {
        Ok(v) => {
            let reference = ["id", "sid", "messageId", "message_id"].iter().find_map(|k| v[k].as_str()).map(str::to_string);
            SendOutcome::Sent(format!("Sent by text from your line {from} to {phone}."), reference)
        }
        // Refused on this machine by the lease gate: nothing left it.
        Err(e @ comm::CommError::Paused) => SendOutcome::PreSendFailure(e.to_string()),
        Err(comm::CommError::Http { status, body }) if (400..500).contains(&status) => {
            SendOutcome::ConfirmedFailure(format!("Could not text from line {from}: NeboAI refused ({status}): {body}. Do not retry with a different resource; tell the owner if this persists."))
        }
        // A server error or no answer: the text may already be on its way.
        Err(e) => SendOutcome::Unknown(format!("texting from line {from}: {e}")),
    })
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_send(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_send(input: &serde_json::Value) -> ToolResult {
    let text = input["text"].as_str().unwrap_or("");
    let phone = input["phone"].as_str().unwrap_or("");

    if text.is_empty() {
        return ToolResult::error(errors::missing_param("send", "text", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
    }
    if phone.is_empty() {
        return ToolResult::error(errors::missing_param("send", "phone", "message(resource: \"sms\", action: \"send\", phone: \"+15551234567\", text: \"Hello!\")"));
    }

    // Use variables and `service id` to avoid quoting issues and work on modern macOS.
    // Pipe via stdin to preserve emoji and multi-byte characters.
    let script = format!(
        "set theMessage to \"{text}\"\n\
         set theBuddy to \"{phone}\"\n\
         tell application \"Messages\"\n\
         \tset targetService to 1st account whose service type = iMessage\n\
         \tset targetBuddy to participant theBuddy of targetService\n\
         \tsend theMessage to targetBuddy\n\
         end tell",
        text = text.replace('\\', "\\\\").replace('"', "\\\""),
        phone = phone.replace('"', "\\\""),
    );
    run_osascript_stdin(
        &script,
        &format!("Handed to Messages.app for delivery to {}", phone),
    )
    .await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_conversations(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_conversations(input: &serde_json::Value) -> ToolResult {
    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let query = format!(
        "SELECT c.chat_identifier, c.display_name, \
         (SELECT COUNT(*) FROM message m JOIN chat_message_join cmj ON m.ROWID = cmj.message_id WHERE cmj.chat_id = c.ROWID) as msg_count, \
         (SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') FROM message m JOIN chat_message_join cmj ON m.ROWID = cmj.message_id WHERE cmj.chat_id = c.ROWID ORDER BY m.date DESC LIMIT 1) as last_message_date \
         FROM chat c ORDER BY last_message_date DESC LIMIT {};",
        limit
    );

    run_sqlite3(&db_path, &query, "No conversations in chat.db (Messages has no chats).").await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_read(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_read(input: &serde_json::Value) -> ToolResult {
    let phone = input["phone"].as_str().unwrap_or("");
    if phone.is_empty() {
        return ToolResult::error(errors::missing_param("read", "phone", "message(resource: \"sms\", action: \"read\", phone: \"+15551234567\")"));
    }

    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let escaped_phone = phone.replace('\'', "''");
    // Media (images/screenshots) is NOT in m.text — it lives in the attachment
    // table; without this column an MMS reads as an empty message (data loss).
    // The filenames are on-disk paths (~/Library/Messages/Attachments/…) the
    // agent can read or view directly.
    let query = format!(
        "SELECT m.is_from_me, \
         datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as msg_date, \
         m.text, \
         (SELECT GROUP_CONCAT(a.filename, ', ') FROM message_attachment_join maj \
          JOIN attachment a ON maj.attachment_id = a.ROWID \
          WHERE maj.message_id = m.ROWID) as attachments \
         FROM message m \
         JOIN chat_message_join cmj ON m.ROWID = cmj.message_id \
         JOIN chat c ON cmj.chat_id = c.ROWID \
         WHERE c.chat_identifier = '{}' \
         ORDER BY m.date DESC LIMIT {};",
        escaped_phone, limit
    );

    run_sqlite3(
        &db_path,
        &query,
        &format!(
            "No messages for chat_identifier '{}' (exact match; list identifiers with action: \"conversations\")",
            phone
        ),
    )
    .await
}

#[cfg(not(target_os = "macos"))]
async fn handle_sms_search(_input: &serde_json::Value) -> ToolResult {
    ToolResult::error("SMS is only available on macOS (via Messages.app). Do not retry on this platform.")
}

#[cfg(target_os = "macos")]
async fn handle_sms_search(input: &serde_json::Value) -> ToolResult {
    let query_text = input["query"].as_str().unwrap_or("");
    if query_text.is_empty() {
        return ToolResult::error(errors::missing_param("search", "query", "message(resource: \"sms\", action: \"search\", query: \"meeting\")"));
    }

    let limit = input["limit"].as_i64().unwrap_or(20);
    let db_path = match chat_db_path() {
        Some(p) => p,
        None => return ToolResult::error("Could not locate ~/Library/Messages/chat.db. Grant Full Disk Access to Nebo in System Settings > Privacy & Security."),
    };

    let escaped_query = query_text.replace('\'', "''");
    let query = format!(
        "SELECT c.chat_identifier, m.is_from_me, \
         datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as msg_date, \
         m.text, \
         (SELECT GROUP_CONCAT(a.filename, ', ') FROM message_attachment_join maj \
          JOIN attachment a ON maj.attachment_id = a.ROWID \
          WHERE maj.message_id = m.ROWID) as attachments \
         FROM message m \
         JOIN chat_message_join cmj ON m.ROWID = cmj.message_id \
         JOIN chat c ON cmj.chat_id = c.ROWID \
         WHERE m.text LIKE '%{}%' \
         ORDER BY m.date DESC LIMIT {};",
        escaped_query, limit
    );

    run_sqlite3(
        &db_path,
        &query,
        &format!("No messages whose text contains '{}' (plain substring).", query_text),
    )
    .await
}

// ---------------------------------------------------------------------------
// Helper: macOS Focus assertions
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helper: macOS chat.db path
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn chat_db_path() -> Option<String> {
    dirs::home_dir()
        .map(|h| h.join("Library/Messages/chat.db"))
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

// ---------------------------------------------------------------------------
// Helper: run sqlite3 CLI
// ---------------------------------------------------------------------------

/// Run `query` against chat.db. `empty` is what an empty result set means
/// for this caller (which identifier or text matched nothing).
#[cfg(target_os = "macos")]
async fn run_sqlite3(db_path: &str, query: &str, empty: &str) -> ToolResult {
    let output = tokio::process::Command::new("sqlite3")
        .args(["-header", "-separator", "|", db_path, query])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok(empty.to_string())
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let hint = if stderr.contains("unable to open database file") || stderr.contains("authorization denied") {
                " Nebo cannot open chat.db: ask the owner to grant Nebo Full Disk Access (System Settings > Privacy & Security > Full Disk Access)."
            } else {
                ""
            };
            ToolResult::error(format!(
                "sqlite3 exited {} reading {}: {}.{}",
                o.status.code().unwrap_or(-1),
                db_path,
                stderr,
                hint
            ))
        }
        Err(e) => ToolResult::error(format!("Failed to run sqlite3: {}. Do not retry — this is a system error.", e)),
    }
}

// ---------------------------------------------------------------------------
// Helper: run osascript (macOS)
// ---------------------------------------------------------------------------

/// Run an AppleScript from stdin. `ok_text` is the result when the script
/// exits 0 without printing anything (what was handed off, to whom).
#[cfg(target_os = "macos")]
async fn run_osascript_stdin(script: &str, ok_text: &str) -> ToolResult {
    use tokio::io::AsyncWriteExt;
    let mut child = match tokio::process::Command::new("osascript")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run osascript: {}. Do not retry — this is a system error.", e)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let output = child.wait_with_output().await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                ToolResult::ok(ok_text.to_string())
            } else {
                ToolResult::ok(stdout)
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            ToolResult::error(format!("osascript error: {}. Do not retry — this is a system error.", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}. Do not retry — this is a system error.", e)),
    }
}
