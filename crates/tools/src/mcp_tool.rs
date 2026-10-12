use std::sync::Arc;

use tracing::{info, warn};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Longest MCP tool description sent to the model; past it the text is cut
/// and marked.
pub const MAX_DESCRIPTION_CHARS: usize = 2_048;

/// An MCP server's tool, exposed as its own tool (`mcp__<server>__<tool>`)
/// with the server's real input schema. Always deferred: a server's
/// `alwaysLoad` would put its tool in the tools array every bot shares and
/// change that array when the server connects, so it is listed and loaded
/// like every other deferred tool. Read-only (and so parallel) only when the
/// server says so.
pub struct McpProxyTool {
    name: String,
    def: mcp::McpToolDef,
    description: String,
    hint: String,
    integration_id: String,
    bridge: Arc<mcp::Bridge>,
    store: Arc<db::Store>,
}

impl McpProxyTool {
    pub fn new(
        name: &str,
        def: &mcp::McpToolDef,
        integration_id: &str,
        bridge: Arc<mcp::Bridge>,
        store: Arc<db::Store>,
    ) -> Self {
        let description = cut_description(&def.description);
        let hint = def.search_hint().unwrap_or_else(|| {
            let words: Vec<String> = name
                .trim_start_matches("mcp__")
                .split(['_', '-'])
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect();
            words.join(" ")
        });
        Self {
            name: name.to_string(),
            def: def.clone(),
            description,
            hint,
            integration_id: integration_id.to_string(),
            bridge,
            store,
        }
    }
}

impl McpProxyTool {
    /// This tool is shared memory's (see [`is_shared_memory`]).
    fn is_shared_memory(&self) -> bool {
        self.store
            .get_mcp_integration(&self.integration_id)
            .ok()
            .flatten()
            .is_some_and(|i| is_shared_memory(&i))
    }
}

/// The integration is the account's shared memory, the Nebo KB: the server
/// at the platform memory URL, matched exactly (a customer's own KB at
/// another host is an ordinary connector). It is wired automatically and
/// managed on the website, so it is never listed as a connector. The ONE
/// predicate for "is this shared memory".
pub fn is_shared_memory(integration: &db::models::McpIntegration) -> bool {
    let memory_url = config::memory_url();
    !memory_url.is_empty() && integration.server_url.as_deref() == Some(memory_url.as_str())
}

/// An MCP description as the model gets it: at most
/// [`MAX_DESCRIPTION_CHARS`], and marked when cut.
fn cut_description(description: &str) -> String {
    if description.chars().count() <= MAX_DESCRIPTION_CHARS {
        return description.to_string();
    }
    let cut: String = description.chars().take(MAX_DESCRIPTION_CHARS).collect();
    format!("{cut}… [truncated]")
}

impl DynTool for McpProxyTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn schema(&self) -> serde_json::Value {
        self.def
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}))
    }

    fn search_hint(&self) -> &str {
        &self.hint
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        self.def.read_only()
    }

    /// A destructive tool's effects are unknown deletes; any other writer's
    /// are unknown.
    fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
        if self.read_only(input) {
            return types::permissions::CallEffects::none();
        }
        let mut effects = types::permissions::CallEffects::unknown();
        if self.def.destructive() {
            effects.deletes.push(self.name.clone());
        }
        effects
    }

    fn max_result_chars(&self, _input: &serde_json::Value) -> Option<usize> {
        Some(self.def.max_result_chars().unwrap_or(crate::registry::DEFAULT_MAX_RESULT_CHARS))
    }

    fn mcp_proxy_info(&self) -> Option<(String, String)> {
        Some((self.integration_id.clone(), self.def.name.clone()))
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        mut input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        // Repair model-stringified object/array args against the server's real
        // schema before forwarding (see coerce_schema_types).
        if let Some(schema) = &self.def.input_schema {
            coerce_schema_types(&mut input, schema);
        }
        // The run's confidentiality scope rides along as a header, so a sealed
        // employee reaches only its own matter on the server side.
        let matter = ctx.memory_matter.clone();
        let domain = input["domain"].as_str().map(str::to_string);
        Box::pin(async move {
            let mut result = call_mcp_tool_scoped(
                &self.store,
                &self.bridge,
                &self.integration_id,
                &self.def.name,
                input,
                matter.as_deref(),
            )
            .await;
            // Shared memory's answers say where they came from, so the model
            // never mistakes them for this Nebo's own memory.
            if !result.is_error && self.is_shared_memory() {
                result.content = format!(
                    "[{}] {}",
                    crate::memory_tools::shared_source(matter.as_deref(), domain.as_deref()),
                    result.content
                );
            }
            result
        })
    }
}

/// Check if a stored OAuth token is expired (with 60s buffer).
pub fn is_token_expired(expires_at: Option<i64>) -> bool {
    token_expires_within(expires_at, 60)
}

/// Whether a stored OAuth token expires within `secs` from now. The proactive
/// refresher uses a wide window so tokens are renewed well before expiry and
/// never reach a 401 at connect time.
pub fn token_expires_within(expires_at: Option<i64>, secs: i64) -> bool {
    match expires_at {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            now >= (exp - secs)
        }
        None => false, // no expiry info = assume valid
    }
}

/// Outcome of resolving an OAuth MCP integration's access token for a connect attempt.
pub enum TokenResolution {
    /// Connect with this token. `None` = non-OAuth / no token needed.
    Ready(Option<String>),
    /// Token is expired and could not be refreshed (refresh failed, no refresh
    /// token, or no stored token). Surface "needs reauth" — do NOT connect with a
    /// stale token, which would 401 and silently drop the server.
    NeedsReauth,
}

/// Resolve the access token to connect an MCP integration with — the single
/// canonical path for startup reconnect, manual connect, sync, and the test
/// button. Refreshes an expired token when possible; on failure returns
/// `NeedsReauth` instead of falling through to the stale token.
pub async fn resolve_mcp_token(
    store: &db::Store,
    client: &mcp::McpClient,
    integration: &db::models::McpIntegration,
) -> TokenResolution {
    if integration.auth_type == "api_key" {
        // Static bearer token — no expiry, no refresh. Missing/undecryptable key
        // surfaces as needs-reauth so Settings → MCP prompts for it.
        return match store.get_mcp_credential_full(&integration.id, "api_key") {
            Ok(Some(cred)) => match client.decrypt_token(&cred.credential_value) {
                Ok(t) => TokenResolution::Ready(Some(t)),
                Err(_) => TokenResolution::NeedsReauth,
            },
            _ => TokenResolution::NeedsReauth,
        };
    }
    if integration.auth_type == "neboai" {
        // Platform-authenticated server (e.g. the Nebo KB): bearer is this
        // Nebo's own NeboAI token, resolved live (`auth::neboai_token`) — never a
        // stored copy, because the token rotates on every comms reconnect and
        // a copy would go stale. No profile = not paired with NeboAI yet.
        return match auth::neboai_token(store) {
            Some(token) => TokenResolution::Ready(Some(token)),
            None => TokenResolution::NeedsReauth,
        };
    }
    if integration.auth_type != "oauth" {
        return TokenResolution::Ready(None);
    }
    let cred = match store.get_mcp_credential_full(&integration.id, "oauth_token") {
        Ok(Some(c)) => c,
        _ => return TokenResolution::NeedsReauth, // OAuth but no stored token
    };
    if !is_token_expired(cred.expires_at) {
        return match client.decrypt_token(&cred.credential_value) {
            Ok(t) => TokenResolution::Ready(Some(t)),
            Err(_) => TokenResolution::NeedsReauth,
        };
    }
    if cred.refresh_token.is_none() {
        return TokenResolution::NeedsReauth;
    }
    match refresh_mcp_token(store, client, &integration.id).await {
        Ok(new_token) => TokenResolution::Ready(Some(new_token)),
        Err(e) => {
            warn!(integration = %integration.id, error = %e, "MCP token refresh failed — needs reauth");
            TokenResolution::NeedsReauth
        }
    }
}

/// Refresh an MCP integration's OAuth token. Orchestrates DB read → decrypt → HTTP refresh →
/// encrypt → DB write → session update. Returns the new plaintext access_token.
pub async fn refresh_mcp_token(
    store: &db::Store,
    client: &mcp::McpClient,
    integration_id: &str,
) -> Result<String, String> {
    // 1. Read OAuth config from integration row
    let oauth_config = store
        .get_mcp_oauth_config(integration_id)
        .map_err(|e| format!("read oauth config: {e}"))?
        .ok_or("no oauth config found")?;

    let token_endpoint = oauth_config
        .oauth_token_endpoint
        .ok_or("no token_endpoint on integration")?;
    let client_id = oauth_config
        .oauth_client_id
        .ok_or("no client_id on integration")?;

    // Decrypt client_secret if present
    let client_secret = oauth_config
        .oauth_client_secret
        .as_deref()
        .and_then(|enc| client.decrypt_token(enc).ok());

    // 2. Read credential with refresh_token
    let cred = store
        .get_mcp_credential_full(integration_id, "oauth_token")
        .map_err(|e| format!("read credential: {e}"))?
        .ok_or("no credential found")?;

    let encrypted_refresh = cred.refresh_token.ok_or("no refresh_token stored")?;
    let refresh_token = client
        .decrypt_token(&encrypted_refresh)
        .map_err(|e| format!("decrypt refresh_token: {e}"))?;

    // 3. Call refresh endpoint
    let result = client
        .refresh_token(
            &token_endpoint,
            &client_id,
            client_secret.as_deref(),
            &refresh_token,
        )
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    // 4. Encrypt and store new tokens
    let new_encrypted_access = client
        .encrypt_token(&result.access_token)
        .map_err(|e| format!("encrypt new access_token: {e}"))?;

    // Use rotated refresh_token if server provided one, otherwise keep old
    let new_encrypted_refresh = match &result.refresh_token {
        Some(new_rt) => Some(
            client
                .encrypt_token(new_rt)
                .map_err(|e| format!("encrypt new refresh_token: {e}"))?,
        ),
        None => Some(encrypted_refresh.clone()),
    };

    let new_expires_at = result.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs
    });

    store
        .store_mcp_credentials(
            integration_id,
            "oauth_token",
            &new_encrypted_access,
            new_encrypted_refresh.as_deref(),
            new_expires_at,
            result.scope.as_deref(),
        )
        .map_err(|e| format!("store new credentials: {e}"))?;

    // 5. Update in-memory session
    let plain_refresh = result.refresh_token.unwrap_or(refresh_token);
    client
        .update_session_token(
            integration_id,
            mcp::OAuthTokens {
                access_token: result.access_token.clone(),
                refresh_token: Some(plain_refresh),
                expires_at: new_expires_at,
                scope: result.scope,
            },
        )
        .await;

    Ok(result.access_token)
}

/// Attempt proactive token refresh if an integration's OAuth token is expired.
/// Returns true if a refresh happened. Shared by the per-tool MCP proxies.
async fn maybe_refresh_token(store: &db::Store, bridge: &mcp::Bridge, integration_id: &str) -> bool {
    let integration = match store.get_mcp_integration(integration_id) {
        Ok(Some(i)) if i.auth_type == "oauth" => i,
        _ => return false,
    };

    let cred = match store.get_mcp_credential_full(&integration.id, "oauth_token") {
        Ok(Some(c)) => c,
        _ => return false,
    };

    if !is_token_expired(cred.expires_at) || cred.refresh_token.is_none() {
        return false;
    }

    info!(integration = integration_id, "MCP token expired, attempting refresh");
    match refresh_mcp_token(store, bridge.client(), integration_id).await {
        Ok(_) => {
            info!(integration = integration_id, "MCP token refreshed");
            true
        }
        Err(e) => {
            warn!(integration = integration_id, error = %e, "MCP token refresh failed");
            false
        }
    }
}

/// Repair model-stringified structured args against the tool's input schema.
///
/// Weaker models (and some OpenAI-compatible gateways) emit object/array-typed
/// parameters as JSON *strings* — `{"policy": "{\"default_route\":...}"}` instead
/// of `{"policy": {...}}` — which the server then rejects as "type string, want
/// object". For each top-level property the schema declares as `object`/`array`,
/// if the model supplied a string that parses to that type, replace it with the
/// parsed value. Values that already match the declared type are left untouched,
/// so a legitimately string-typed field is never mangled.
pub(crate) fn coerce_schema_types(input: &mut serde_json::Value, schema: &serde_json::Value) {
    let (Some(obj), Some(props)) = (
        input.as_object_mut(),
        schema.get("properties").and_then(|p| p.as_object()),
    ) else {
        return;
    };
    for (key, val) in obj.iter_mut() {
        let serde_json::Value::String(s) = val else {
            continue;
        };
        let Some(types) = props.get(key).and_then(|p| p.get("type")) else {
            continue;
        };
        let accepts = |t: &str| match types {
            serde_json::Value::String(one) => one == t,
            serde_json::Value::Array(many) => many.iter().any(|v| v.as_str() == Some(t)),
            _ => false,
        };
        let wants_object = accepts("object");
        let wants_array = accepts("array");
        if !wants_object && !wants_array {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            if (wants_object && parsed.is_object()) || (wants_array && parsed.is_array()) {
                *val = parsed;
            }
            continue;
        }
        // The most common LLM serialization fault: raw control characters
        // (real newlines/tabs) inside string literals — invalid JSON that
        // left the value an opaque string and sent models into identical-call
        // retry spirals. Repair by escaping control chars inside strings.
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&escape_control_chars(s)) {
            if (wants_object && parsed.is_object()) || (wants_array && parsed.is_array()) {
                *val = parsed;
            }
        }
    }
}

/// Escape raw control characters that appear INSIDE string literals of an
/// almost-JSON document. Leaves structure whitespace untouched.
pub(crate) fn escape_control_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_str = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_str {
            if escaped {
                out.push(c);
                escaped = false;
                continue;
            }
            match c {
                '\\' => {
                    out.push(c);
                    escaped = true;
                }
                '"' => {
                    out.push(c);
                    in_str = false;
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
        }
    }
    out
}

/// Canonical MCP tool execution: proactive token refresh, call via the bridge, and a
/// single 401-retry that refreshes the token before retrying. This is THE call pathway
/// for MCP tools — invoked by the per-tool proxy tools (`McpProxyTool`). The input is
/// forwarded verbatim to the underlying tool (no argument stripping), so a tool whose
/// own schema requires `resource`/`action` receives them unchanged.
pub async fn call_mcp_tool(
    store: &db::Store,
    bridge: &mcp::Bridge,
    integration_id: &str,
    tool_name: &str,
    input: serde_json::Value,
) -> ToolResult {
    call_mcp_tool_scoped(store, bridge, integration_id, tool_name, input, None).await
}

/// As `call_mcp_tool`, carrying the run's confidentiality scope. A call the
/// server refuses for its arguments (invalid params, an unknown field) may
/// have been made against a stale definition: the server's tools are listed
/// again and, when this tool's definition changed, the call is tried once
/// more against its new schema.
pub async fn call_mcp_tool_scoped(
    store: &db::Store,
    bridge: &mcp::Bridge,
    integration_id: &str,
    tool_name: &str,
    input: serde_json::Value,
    matter: Option<&str>,
) -> ToolResult {
    let first = call_once(store, bridge, integration_id, tool_name, input.clone(), matter).await;
    if !first.is_error || !mcp::client::is_schema_error(&first.content) {
        return first;
    }
    let relisted = match bridge.refresh(integration_id).await {
        Ok(relisted) => relisted,
        Err(e) => {
            warn!(integration = %integration_id, tool = tool_name, error = %e, "connector tools could not be listed again");
            return first;
        }
    };
    if !relisted.changed.iter().any(|n| n == tool_name) {
        return first;
    }
    let Some(def) = relisted.tools.iter().find(|d| d.name == tool_name) else {
        return ToolResult::error(format!(
            "{}\nThis connector no longer offers {tool_name}.",
            first.content
        ));
    };
    info!(integration = %integration_id, tool = tool_name, "connector tool changed; trying the call again");
    let mut input = input;
    if let Some(schema) = &def.input_schema {
        coerce_schema_types(&mut input, schema);
    }
    call_once(store, bridge, integration_id, tool_name, input, matter).await
}

/// One call: proactive token refresh, the call, and one retry on a 401.
async fn call_once(
    store: &db::Store,
    bridge: &mcp::Bridge,
    integration_id: &str,
    tool_name: &str,
    input: serde_json::Value,
    matter: Option<&str>,
) -> ToolResult {
    // Proactive refresh: if the token is expired, refresh before calling.
    maybe_refresh_token(store, bridge, integration_id).await;

    match bridge.call_tool_scoped(integration_id, tool_name, input.clone(), matter).await {
        Ok(result) => {
            if result.is_error {
                ToolResult::error(result.content)
            } else {
                ToolResult::ok(result.content)
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            // Retry once on 401 (token may have been revoked server-side before expiry).
            if err_str.contains("401") || err_str.contains("Unauthorized") {
                info!(integration = %integration_id, "MCP 401, attempting token refresh");
                match refresh_mcp_token(store, bridge.client(), integration_id).await {
                    Ok(_) => match bridge.call_tool(integration_id, tool_name, input).await {
                        Ok(result) => {
                            if result.is_error {
                                ToolResult::error(result.content)
                            } else {
                                ToolResult::ok(result.content)
                            }
                        }
                        Err(retry_err) => ToolResult::error(format!(
                            "Connector {}/{} failed after token refresh: {}",
                            integration_id, tool_name, retry_err
                        )),
                    },
                    Err(refresh_err) => {
                        let _ =
                            store.set_mcp_connection_status(integration_id, "disconnected", 0, None);
                        // Shared memory has no Connectors entry to send the
                        // owner to: the website manages it.
                        let shared = store
                            .get_mcp_integration(integration_id)
                            .ok()
                            .flatten()
                            .is_some_and(|i| is_shared_memory(&i));
                        if shared {
                            return ToolResult::error(
                                "Shared memory isn't available to this Nebo right now. Carry on without it.",
                            );
                        }
                        ToolResult::error(format!(
                            "Connector {}/{}: sign-in expired and refresh failed: {}. Ask the owner to reconnect it in Settings > Connectors.",
                            integration_id, tool_name, refresh_err
                        ))
                    }
                }
            } else {
                ToolResult::error(format!("Connector {}/{} failed: {}", integration_id, tool_name, e))
            }
        }
    }
}

#[cfg(test)]
mod neboai_token_tests {
    use super::{resolve_mcp_token, TokenResolution};
    use std::sync::Arc;

    fn kb_integration() -> db::models::McpIntegration {
        db::models::McpIntegration {
            id: "kb-test".into(),
            name: "nebo-kb".into(),
            server_type: "http".into(),
            server_url: Some("https://kb.example".into()),
            auth_type: "neboai".into(),
            is_enabled: Some(1),
            connection_status: None,
            last_connected_at: None,
            last_error: None,
            metadata: None,
            created_at: 0,
            updated_at: 0,
            tool_count: None,
            artifact_id: None,
        }
    }

    #[tokio::test]
    async fn neboai_auth_resolves_live_profile_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let client = mcp::McpClient::new(Arc::new(mcp::crypto::Encryptor::generate()));

        // No NeboAI profile → NeedsReauth, never Ready(None) (a tokenless
        // connect to a platform-authed server would 401 and drop the server).
        assert!(matches!(
            resolve_mcp_token(&store, &client, &kb_integration()).await,
            TokenResolution::NeedsReauth
        ));

        store
            .create_auth_profile(
                "p1", "NeboAI", "neboai", "live-token", None, None, 0, 1, Some("token"), None,
            )
            .unwrap();
        match resolve_mcp_token(&store, &client, &kb_integration()).await {
            TokenResolution::Ready(Some(t)) => assert_eq!(t, "live-token"),
            _ => panic!("expected the live NeboAI profile token"),
        }
    }
}

#[cfg(test)]
mod coerce_tests {
    use super::{coerce_schema_types, escape_control_chars};

    // The exact spiral from the field: `tasks` as a string whose prompt values
    // contain REAL newlines — invalid JSON the silent coercion used to skip.
    #[test]
    fn repairs_raw_newlines_inside_string_literals() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "tasks": { "type": "array" } }
        });
        let mut input = serde_json::json!({
            "tasks": "\n[{\"prompt\": \"line one\nline two\"}]"
        });
        coerce_schema_types(&mut input, &schema);
        assert!(input["tasks"].is_array(), "repaired string must coerce to array");
        assert_eq!(input["tasks"][0]["prompt"], "line one\nline two");
        // structure whitespace stays untouched; escapes inside strings survive
        let fixed = escape_control_chars("{\"a\": \"x\\ny\"}");
        assert_eq!(fixed, "{\"a\": \"x\\ny\"}");
    }
    use serde_json::json;

    #[test]
    fn parses_stringified_object_and_array_leaves_strings_alone() {
        let schema = json!({
            "properties": {
                "policy": {"type": ["null", "object"]},
                "tags":   {"type": "array"},
                "key":    {"type": "string"},
            }
        });
        let mut input = json!({
            "policy": "{\"default_route\":{\"provider\":\"dashscope\",\"model\":\"glm-5.2\"}}",
            "tags":   "[\"a\",\"b\"]",
            "key":    "{\"not\":\"parsed\"}",
        });
        coerce_schema_types(&mut input, &schema);

        assert_eq!(input["policy"]["default_route"]["model"], "glm-5.2");
        assert!(input["tags"].is_array());
        // string-typed field is never mangled, even though it looks like JSON
        assert_eq!(input["key"], "{\"not\":\"parsed\"}");
    }

    #[test]
    fn already_correct_object_is_untouched() {
        let schema = json!({"properties": {"policy": {"type": "object"}}});
        let mut input = json!({"policy": {"default_route": {"model": "x"}}});
        let before = input.clone();
        coerce_schema_types(&mut input, &schema);
        assert_eq!(input, before);
    }
}

#[cfg(test)]
mod matter_scope_tests {
    use crate::origin::{Origin, ToolContext};

    /// The confidentiality scope must ride on the ToolContext — the run's
    /// property, not an argument the model authors. A model that could name
    /// its own matter could name another client's.
    #[test]
    fn matter_lives_on_the_context_not_the_arguments() {
        let plain = ToolContext::new(Origin::Workflow);
        assert!(plain.memory_matter.is_none(), "a normal employee is unscoped");

        let sealed = ToolContext {
            memory_matter: Some("matter/acme-v-smith".into()),
            ..ToolContext::new(Origin::Workflow)
        };
        assert_eq!(sealed.memory_matter.as_deref(), Some("matter/acme-v-smith"));
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    fn def(json: serde_json::Value) -> mcp::McpToolDef {
        serde_json::from_value(json).unwrap()
    }

    /// The rules for an MCP tool: the description cut at 2,048
    /// characters, `readOnlyHint` makes it read-only (and so parallel),
    /// `_meta` can load it always, name its search words and its result
    /// size; with no hints it is a deferred writer.
    #[tokio::test]
    async fn a_proxy_follows_the_servers_annotations_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap());
        let registry = Arc::new(crate::registry::Registry::new(crate::gate::test_gate()));
        let client = Arc::new(mcp::McpClient::new(Arc::new(mcp::crypto::Encryptor::generate())));
        let bridge = Arc::new(mcp::Bridge::new(client, registry));
        let proxy = |d: &mcp::McpToolDef| McpProxyTool::new("mcp__docs__search", d, "int-1", bridge.clone(), store.clone());

        let plain = proxy(&def(serde_json::json!({"name": "search", "description": "x".repeat(3_000)})));
        assert!(plain.should_defer());
        assert!(!plain.read_only(&serde_json::json!({})) && !plain.concurrency_safe(&serde_json::json!({})));
        assert_eq!(plain.description().chars().count(), MAX_DESCRIPTION_CHARS + "… [truncated]".chars().count());
        assert!(plain.description().ends_with("… [truncated]"));
        assert_eq!(plain.search_hint(), "docs search");
        assert_eq!(plain.max_result_chars(&serde_json::json!({})), Some(crate::registry::DEFAULT_MAX_RESULT_CHARS));

        let hinted = proxy(&def(serde_json::json!({
            "name": "search", "description": "Searches the docs.",
            "annotations": {"readOnlyHint": true},
            "_meta": {"anthropic/alwaysLoad": true, "anthropic/searchHint": "find docs\n pages", "anthropic/maxResultSizeChars": 20000}
        })));
        assert!(hinted.should_defer(), "alwaysLoad never puts a server's tool in the shared tools array");
        assert!(hinted.read_only(&serde_json::json!({})) && hinted.concurrency_safe(&serde_json::json!({})));
        assert_eq!(hinted.description(), "Searches the docs.");
        assert_eq!(hinted.search_hint(), "find docs pages");
        assert_eq!(hinted.max_result_chars(&serde_json::json!({})), Some(20_000));

        let destructive = proxy(&def(serde_json::json!({"name": "drop", "annotations": {"destructiveHint": true}})));
        assert_eq!(destructive.effects(&serde_json::json!({})).deletes, vec!["mcp__docs__search".to_string()]);
    }
}

#[cfg(test)]
mod refresh_tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::origin::{Origin, ToolContext};

    /// A remote MCP server whose tool list the test changes under it, as
    /// Janus added `display_names` to `prices`. A call with a field its
    /// current schema lacks is refused as invalid params; `notify` streams
    /// `notifications/tools/list_changed` ahead of the next call's answer.
    #[derive(Default)]
    struct Server {
        tools: Value,
        calls: Vec<Value>,
        notify: bool,
    }

    fn prices(fields: &[&str]) -> Value {
        let props: serde_json::Map<String, Value> =
            fields.iter().map(|f| (f.to_string(), json!({"type": "string"}))).collect();
        json!([{"name": "prices", "description": "Prices.", "inputSchema": {
            "type": "object", "properties": props, "additionalProperties": false
        }}])
    }

    fn answer(server: &Mutex<Server>, req: &Value) -> (String, &'static str) {
        let mut s = server.lock().unwrap();
        let id = req["id"].clone();
        let body = match req["method"].as_str() {
            Some("tools/list") => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": s.tools}}),
            Some("tools/call") => {
                let args = req["params"]["arguments"].clone();
                s.calls.push(args.clone());
                let known = s.tools[0]["inputSchema"]["properties"].clone();
                match args.as_object().into_iter().flatten().find(|(k, _)| known.get(k.as_str()).is_none()) {
                    Some((k, _)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("unknown field `{k}`")}}),
                    None => json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "priced"}]}}),
                }
            }
            _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        };
        if std::mem::take(&mut s.notify) {
            let note = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
            return (format!("event: message\ndata: {note}\n\nevent: message\ndata: {body}\n\n"), "text/event-stream");
        }
        (body.to_string(), "application/json")
    }

    async fn serve(server: Arc<Mutex<Server>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let server = server.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                        let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if buf.len() < end + 4 + len {
                            continue;
                        }
                        let req: Value = serde_json::from_slice(&buf[end + 4..end + 4 + len]).unwrap_or_default();
                        let (body, kind) = answer(&server, &req);
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        return;
                    }
                });
            }
        });
        format!("http://{addr}/mcp")
    }

    struct World {
        server: Arc<Mutex<Server>>,
        registry: Arc<crate::registry::Registry>,
        bridge: Arc<mcp::Bridge>,
        store: Arc<db::Store>,
        _dir: tempfile::TempDir,
    }

    /// A registry with the server connected as integration `int-1`, its
    /// tools under `mcp__janus__*`.
    async fn world(fields: &[&str]) -> World {
        let server = Arc::new(Mutex::new(Server { tools: prices(fields), ..Default::default() }));
        let url = serve(server.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap());
        store.create_mcp_integration("int-1", "janus", "http", Some(&url), "none", None, None).unwrap();
        let registry = Arc::new(crate::registry::Registry::new(crate::gate::test_gate()));
        let client = Arc::new(mcp::McpClient::new(Arc::new(mcp::crypto::Encryptor::generate())));
        let bridge = Arc::new(mcp::Bridge::new(client, registry.clone()));
        registry.set_bridge(bridge.clone());
        registry.set_store(store.clone());
        bridge.watch_tool_changes();
        bridge.connect("int-1", "janus", &url, None, None).await.unwrap();
        World { server, registry, bridge, store, _dir: dir }
    }

    fn props(def: &ai::ToolDefinition) -> Vec<String> {
        def.input_schema["properties"].as_object().unwrap().keys().cloned().collect()
    }

    /// Refresh lists the server's tools again on the live connection and
    /// re-registers what changed; listing an unchanged server changes
    /// nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_registers_the_servers_new_schema() {
        let w = world(&["ids"]).await;
        let unchanged = w.bridge.refresh("int-1").await.unwrap();
        assert!(unchanged.changed.is_empty());
        w.server.lock().unwrap().tools = prices(&["ids", "display_names"]);
        let relisted = w.bridge.refresh("int-1").await.unwrap();
        assert_eq!(relisted.changed, vec!["prices"]);
        let def = w.registry.definition("mcp__janus__prices").await.unwrap();
        assert_eq!(props(&def), vec!["display_names", "ids"]);
    }

    /// The Janus case: the server added `display_names`, the registry still
    /// held the old schema and refused the field as unknown. The refusal
    /// lists the tools again and the call runs with the field.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_refused_by_a_stale_schema_runs_after_the_tools_are_listed_again() {
        let w = world(&["ids"]).await;
        w.server.lock().unwrap().tools = prices(&["ids", "display_names"]);
        let ctx = ToolContext::new(Origin::User);
        let result = w
            .registry
            .execute(&ctx, "mcp__janus__prices", json!({"ids": "a", "display_names": "yes"}))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.content, "priced");
        assert_eq!(w.server.lock().unwrap().calls, vec![json!({"ids": "a", "display_names": "yes"})]);
    }

    /// A field the server refuses as invalid params (-32602) while the
    /// local schema allowed it: the tools are listed again, and the call is
    /// tried once more only when the tool's definition changed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_schema_error_lists_the_tools_again_and_retries_once() {
        let w = world(&["ids", "currency"]).await;
        let store = w.store.clone();
        // The server dropped `currency` and added `display_names`; the call
        // passes the old schema locally and the server refuses it.
        w.server.lock().unwrap().tools = prices(&["ids", "display_names"]);
        let result =
            super::call_mcp_tool(&store, &w.bridge, "int-1", "prices", json!({"ids": "a", "currency": "usd"})).await;
        assert!(result.is_error, "the field is gone, so the retry is refused too");
        assert_eq!(w.server.lock().unwrap().calls.len(), 2, "one call, one retry");
        let def = w.registry.definition("mcp__janus__prices").await.unwrap();
        assert_eq!(props(&def), vec!["display_names", "ids"]);

        // Unchanged definitions: no retry.
        let again =
            super::call_mcp_tool(&store, &w.bridge, "int-1", "prices", json!({"ids": "a", "currency": "usd"})).await;
        assert!(again.is_error);
        assert_eq!(w.server.lock().unwrap().calls.len(), 3);
    }

    /// `notifications/tools/list_changed` from the server lists its tools
    /// again without anyone asking.
    #[tokio::test(flavor = "multi_thread")]
    async fn list_changed_from_the_server_lists_the_tools_again() {
        let w = world(&["ids"]).await;
        {
            let mut s = w.server.lock().unwrap();
            s.tools = prices(&["ids", "display_names"]);
            s.notify = true;
        }
        let store = w.store.clone();
        let result = super::call_mcp_tool(&store, &w.bridge, "int-1", "prices", json!({"ids": "a"})).await;
        assert!(!result.is_error, "{}", result.content);
        for _ in 0..100 {
            let def = w.registry.definition("mcp__janus__prices").await.unwrap();
            if props(&def).len() == 2 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the tools were not listed again after list_changed");
    }
}
