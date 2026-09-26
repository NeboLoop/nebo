//! `recall`, `remember` and `forget`: the employee's memory. Always loaded,
//! since memory is used on almost every turn. Isolation, the provenance write
//! bar, recall-for-audience and credential routing are the store's rules and
//! hold for every call.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::bot_tool::{HybridSearcher, KeychainStore, MemoryEmbedder, OsKeychain};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Keychain service name for credential-routed memories (Phase 1 of
/// docs/plans/memory-rock-solid.md). Account = "{memory scope}/{key}" — the
/// scope prefix keeps the secret store as isolated as the memory row that
/// points at it: with a bare key, matter B storing the same key silently
/// overwrote matter A's secret and any scope could read it back (isolation
/// audit 2026-08-22, leak #7).
const MEMORY_KEYCHAIN_SERVICE: &str = "nebo-memory";

/// Where a fact is looked up by key when the call names no namespace.
const DEFAULT_NAMESPACE: &str = "tacit/general";

/// What `recall` with no query lists.
const LIST_PREFIX: &str = "tacit/";

/// The memory the three tools share.
pub struct Memory {
    store: Arc<Store>,
    hybrid_searcher: Option<Arc<dyn HybridSearcher>>,
    embedder: Option<Arc<dyn MemoryEmbedder>>,
    keychain: Arc<dyn KeychainStore>,
}

impl Memory {
    pub fn new(
        store: Arc<Store>,
        hybrid_searcher: Option<Arc<dyn HybridSearcher>>,
        embedder: Option<Arc<dyn MemoryEmbedder>>,
    ) -> Self {
        Self {
            store,
            hybrid_searcher,
            embedder,
            keychain: Arc::new(OsKeychain),
        }
    }

    /// Replace the OS keychain used for credential routing (tests inject a
    /// recording stub; production keeps the OS keychain).
    pub fn with_keychain(mut self, keychain: Arc<dyn KeychainStore>) -> Self {
        self.keychain = keychain;
        self
    }

    /// The three memory tools over one memory.
    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let memory = Arc::new(self);
        [MemoryOp::Recall, MemoryOp::Remember, MemoryOp::Forget]
            .into_iter()
            .map(|op| {
                Box::new(MemoryTool {
                    op,
                    memory: memory.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    /// Fail-closed isolation (set by the runner's scope derivation): a
    /// context-isolated employee with no derivable context must not change
    /// the shared scope. Reads still serve the inherited chain.
    fn writes_refused(ctx: &ToolContext) -> Option<ToolResult> {
        ctx.memory_writes_disabled.then(|| {
            ToolResult::error(
                "Memory writes are disabled for this run: this employee's memory is kept per \
                 conversation and no conversation could be derived. recall still works.",
            )
        })
    }

    /// Recall-for-audience (trust-boundaries design 2026-08-22): replying to
    /// a coworker not granted by `memory.share_with`, memory lookups are
    /// refused outright. Working style (`tacit/`) already reaches the model
    /// through the prompt, filtered to what may be shared.
    fn reads_refused(ctx: &ToolContext) -> Option<ToolResult> {
        ctx.audience_restricted.then(|| {
            ToolResult::error(
                "Memory lookup is disabled while replying to a coworker who is not granted \
                 access to this scope. Answer from the context you already have, or tell them the \
                 information isn't shared with their role. Do not retry.",
            )
        })
    }

    async fn remember(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        if let Some(refused) = Self::writes_refused(ctx) {
            return refused;
        }
        // Provenance write bar (trust-boundaries design 2026-08-22): a run
        // whose engine-stamped taint intersects this scope's bar must not
        // store — pollution stops at the store, not at the model's judgment.
        let barred: Vec<_> = ctx
            .run_taint
            .iter()
            .filter(|c| ctx.memory_write_bar.contains(c))
            .copied()
            .collect();
        if !barred.is_empty() {
            return ToolResult::error(format!(
                "Not saved: this memory scope refuses content from runs that touched {} (scope \
                 write bar). Relay the information to the owner instead of storing it. Do not \
                 retry.",
                types::provenance::label_classes(&barred)
            ));
        }

        let key = input["key"].as_str().unwrap_or("");
        let value = input["value"].as_str().unwrap_or("");
        // `layer` maps to the namespace for that layer; an explicit
        // `namespace` overrides. The `daily` layer is retired
        // (docs/design/MEMORY_QUALITY.md) — ongoing work lives in the topical
        // `project` layer, plus any topics the employee declared.
        let layer_ns = match input["layer"].as_str().unwrap_or("") {
            "entity" => "entity/default".to_string(),
            "project" => "project".to_string(),
            l if ctx.memory_topics.iter().any(|t| t == l) => l.to_string(),
            _ => DEFAULT_NAMESPACE.to_string(),
        };
        let namespace = input["namespace"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or(&layer_ns);

        // Deterministic credential routing (memory-rock-solid Phase 1): an
        // explicit store of a credential-shaped value is never refused and
        // never stored plaintext — the secret goes to the OS keychain and the
        // row keeps a pointer. Same classifier stage-0 uses on extraction.
        let (stored_value, keychain_kind) = match crate::memory_guard::classify_credential(value) {
            Some(kind) => {
                if let Err(e) = self
                    .keychain
                    .store(
                        MEMORY_KEYCHAIN_SERVICE,
                        &format!("{}/{}", ctx.user_id, key),
                        value,
                    )
                    .await
                {
                    warn!(kind = kind, key = key, error = %e, "credential routing: keychain write failed");
                    // Never fall back to plaintext after deciding it is a
                    // credential — refuse this store instead.
                    return ToolResult::error(format!(
                        "Not saved: the value is credential-shaped ({kind}) and the OS keychain \
                         write failed ({e}), so it cannot be stored safely. Plaintext credentials \
                         are never written to memory. Tell the owner the keychain write failed; \
                         do not retry with the same value."
                    ));
                }
                (
                    format!(
                        "(stored in system keychain: {MEMORY_KEYCHAIN_SERVICE}, account {}/{key})",
                        ctx.user_id
                    ),
                    Some(kind),
                )
            }
            None => {
                // Stage-0 write guard — the filter automatic extraction runs.
                // Explicit stores are owner-directed, so short stated facts
                // ("favorite color: blue") survive the too-thin rule.
                if let Some(rule) = crate::memory_guard::stage0_reject(key, value, true) {
                    warn!(
                        rule = rule,
                        key = key,
                        "memory store rejected by stage-0 guard"
                    );
                    return ToolResult::error(format!(
                        "Not saved ({rule}): this value is not a durable fact worth remembering \
                         (secrets, bare numbers/times/paths, and session mechanics are filtered). \
                         Save a self-contained 1-2 sentence fact instead, or skip it."
                    ));
                }
                (value.to_string(), None)
            }
        };

        debug!(namespace, key, value_len = stored_value.len(), user_id = %ctx.user_id, "memory store attempt");

        // Provenance rides the metadata annex — the classes of untrusted
        // content the storing run touched (empty = clean).
        let provenance_meta =
            (!ctx.run_taint.is_empty()).then(|| json!({ "provenance": ctx.run_taint }).to_string());
        if let Err(e) = self.store.upsert_memory(
            namespace,
            key,
            &stored_value,
            None,
            provenance_meta.as_deref(),
            &ctx.user_id,
        ) {
            return ToolResult::error(format!(
                "Failed to save memory [{namespace}] {key}: {e}. Do not retry immediately — this \
                 is a database error, not a parameter issue."
            ));
        }
        // Verify the write on a different pool connection.
        match self
            .store
            .get_memory_by_key_and_user(namespace, key, &ctx.user_id)
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                let total = self.store.count_memories().ok();
                warn!(namespace, key, user_id = %ctx.user_id, total_memories = total.unwrap_or(-1),
                    "memory store: upsert OK but cross-connection verify found NOTHING");
                return ToolResult::error(format!(
                    "Memory save failed: the write to [{namespace}] {key} was accepted but could \
                     not be read back; nothing was saved.{} Do not retry; tell the owner memory \
                     storage is failing.",
                    total
                        .map(|t| format!(" Memories in DB: {t}."))
                        .unwrap_or_default()
                ));
            }
            Err(e) => warn!(key, error = %e, "memory store verify read failed"),
        }
        // Explicit stores get the same background chunk+embed treatment as
        // automatic extraction, so vector recall finds them too.
        if let Some(ref embedder) = self.embedder {
            embedder.embed(namespace, key, &ctx.user_id);
        }
        match keychain_kind {
            Some(kind) => ToolResult::ok(format!(
                "Saved a pointer for {key} in [{namespace}]; the value was credential-shaped \
                 ({kind}), and the secret itself is in the OS keychain (service \
                 {MEMORY_KEYCHAIN_SERVICE}, account {}/{key}). Tell the owner where it lives.",
                ctx.user_id
            )),
            None => ToolResult::ok(format!("Remembered: [{namespace}] {key} = {stored_value}")),
        }
    }

    /// A query that is a stored key returns that fact; any other query
    /// searches; no query lists recent memories.
    async fn recall(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        if let Some(refused) = Self::reads_refused(ctx) {
            return refused;
        }
        let query = input["query"].as_str().map(str::trim).unwrap_or("");
        let namespace = input["namespace"].as_str().filter(|n| !n.is_empty());
        if query.is_empty() {
            let limit = input["limit"].as_i64().unwrap_or(50);
            return self.list(namespace.unwrap_or(LIST_PREFIX), limit, ctx);
        }
        match self.find_by_key(namespace.unwrap_or(DEFAULT_NAMESPACE), query, ctx) {
            Ok(Some(found)) => return ToolResult::ok(found),
            Ok(None) => {}
            Err(e) => return ToolResult::error(e),
        }
        let limit = input["limit"].as_i64().unwrap_or(20) as usize;
        self.search(query, limit, ctx).await
    }

    /// The fact stored under `key`: in `namespace` for this scope first, then
    /// under an ancestor scope, then in any namespace. Sibling employees and
    /// sibling isolation contexts are never readable.
    fn find_by_key(
        &self,
        namespace: &str,
        key: &str,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        match self
            .store
            .get_memory_by_key_and_user(namespace, key, &ctx.user_id)
        {
            Ok(Some(mem)) => {
                let _ = self
                    .store
                    .increment_memory_access_by_key(namespace, key, &ctx.user_id);
                return Ok(Some(format!(
                    "[{}] {}: {}",
                    mem.namespace, mem.key, mem.value
                )));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(format!(
                    "Failed to recall memory [{namespace}] {key}: {e}. Do not retry — this is a \
                     database error."
                ));
            }
        }
        // Legacy owner-scoped memories: only ANCESTOR scopes are readable.
        if let Some(m) = self
            .store
            .get_memory_by_key(namespace, key)
            .ok()
            .flatten()
            .filter(|m| scope_is_ancestor(&ctx.user_id, &m.user_id))
        {
            warn!(namespace, key, expected_user_id = %ctx.user_id, actual_user_id = %m.user_id,
                "memory found under ancestor scope — returning");
            let _ = self
                .store
                .increment_memory_access_by_key(namespace, key, &m.user_id);
            return Ok(Some(format!(
                "[{}] {}: {} (inherited from owner scope)",
                m.namespace, m.key, m.value
            )));
        }
        // Key-only lookup across namespaces (same ancestor-scope guard).
        Ok(self
            .store
            .find_memory_by_key(key)
            .ok()
            .flatten()
            .filter(|m| scope_is_ancestor(&ctx.user_id, &m.user_id))
            .map(|m| {
                let inherited = if m.user_id == ctx.user_id {
                    ""
                } else {
                    "; inherited from owner scope"
                };
                format!(
                    "[{}] {}: {} (found in namespace {}{inherited})",
                    m.namespace, m.key, m.value, m.namespace
                )
            }))
    }

    async fn search(&self, query: &str, limit: usize, ctx: &ToolContext) -> ToolResult {
        // Hybrid search (FTS5 + vector) when available.
        if let Some(ref searcher) = self.hybrid_searcher {
            let results = searcher.search(query, &ctx.user_id, limit, None).await;
            if !results.is_empty() {
                let lines: Vec<String> = results
                    .iter()
                    .map(|r| {
                        format!(
                            "- [{}] {}: {} (relevance {:.2}/1)",
                            r.namespace, r.key, r.value, r.score
                        )
                    })
                    .collect();
                return ToolResult::ok(format!(
                    "Found {} memories (semantic search):\n{}",
                    results.len(),
                    lines.join("\n")
                ));
            }
        }
        match self
            .store
            .search_memories_by_user(&ctx.user_id, query, limit as i64, 0)
        {
            Ok(memories) if memories.is_empty() => {
                ToolResult::ok(format!("No memories found matching: {query}"))
            }
            Ok(memories) => {
                let lines: Vec<String> = memories
                    .iter()
                    .map(|m| format!("- [{}] {}: {}", m.namespace, m.key, m.value))
                    .collect();
                ToolResult::ok(format!(
                    "Found {} memories (text match):\n{}",
                    memories.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!(
                "Memory search failed: {e}. Do not retry — this is a database error. Call recall \
                 with no query to list memories instead."
            )),
        }
    }

    /// Always scoped to this employee — never another employee's memories.
    fn list(&self, prefix: &str, limit: i64, ctx: &ToolContext) -> ToolResult {
        match self
            .store
            .list_memories_by_user_and_namespace(&ctx.user_id, prefix, limit, 0)
        {
            Ok(mems) if mems.is_empty() => ToolResult::ok(format!(
                "No memories in namespace prefix '{prefix}'. (With no namespace this lists tacit/; \
                 pass namespace: \"project\" or \"entity/\" to see others.)"
            )),
            Ok(mems) => {
                let lines: Vec<String> = mems
                    .iter()
                    .map(|m| format!("- [{}] {}: {}", m.namespace, m.key, m.value))
                    .collect();
                let page_note = if mems.len() as i64 >= limit {
                    format!(" (first {limit}; raise limit for more)")
                } else {
                    String::new()
                };
                ToolResult::ok(format!(
                    "{} memories in {prefix}{page_note}:\n{}",
                    mems.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to list memories: {e}")),
        }
    }

    fn forget(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        if let Some(refused) = Self::writes_refused(ctx) {
            return refused;
        }
        let key = input["key"].as_str().unwrap_or("");
        let namespace = input["namespace"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or(DEFAULT_NAMESPACE);
        // Scoped to this employee only — never another's.
        match self
            .store
            .delete_memory_by_key_and_user(namespace, key, &ctx.user_id)
        {
            Ok(n) if n > 0 => ToolResult::ok(format!(
                "Forgot {n} entries for key '{key}' in {namespace}."
            )),
            Ok(_) => ToolResult::ok(format!(
                "Nothing forgotten: no memory with key '{key}' in {namespace}. recall with the key \
                 shows the namespace it is in."
            )),
            Err(e) => ToolResult::error(format!("Failed to forget: {e}")),
        }
    }
}

/// True when `found` is `own` itself or an ANCESTOR scope of `own` in the
/// memory scope chain (`owner` → `owner:agent:X` → `owner:agent:X:ctx:Y` —
/// scopes nest with `:` separators, so an ancestor is a strict `:`-boundary
/// prefix). Sibling employees and sibling isolation contexts never are.
fn scope_is_ancestor(own: &str, found: &str) -> bool {
    own == found || own.starts_with(&format!("{found}:"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryOp {
    Recall,
    Remember,
    Forget,
}

struct MemoryTool {
    op: MemoryOp,
    memory: Arc<Memory>,
}

impl DynTool for MemoryTool {
    fn name(&self) -> &str {
        match self.op {
            MemoryOp::Recall => "recall",
            MemoryOp::Remember => "remember",
            MemoryOp::Forget => "forget",
        }
    }

    fn description(&self) -> String {
        match self.op {
            MemoryOp::Recall => "Searches what you've remembered about the owner, the company and past work.\n\
                 - `query` can be a saved key (returns that fact) or words to search for.\n\
                 - Leave `query` empty to list recent memories; `namespace` narrows the list (\"project\", \"entity/\").\n\
                 - A fact that reads \"(stored in system keychain: …)\" is a pointer: fetch the secret only when the owner asks for it."
                .to_string(),
            MemoryOp::Remember => "Saves a fact worth keeping across conversations. Use a short, specific key (\"owner/coffee-order\").\n\
                 - `layer`: \"tacit\" for preferences and working style (the default), \"project\" for ongoing work, \"entity\" for people, places and things.\n\
                 - Use the owner's exact words; don't paraphrase.\n\
                 - When the owner asks you to remember something, save it — the request is their consent. Passwords and API keys go to the system keychain and the memory keeps a pointer; the result says so.\n\
                 - Saving to an existing key replaces it."
                .to_string(),
            MemoryOp::Forget => "Deletes a remembered fact by its key.\n\
                 - `namespace` defaults to tacit/general; recall with the key shows where a fact lives."
                .to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            MemoryOp::Recall => json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "A saved key, or words to search for. Leave out to list recent memories." },
                    "namespace": { "type": "string", "description": "Namespace to look in, e.g. \"tacit/preferences\", \"project\", \"entity/\"." },
                    "limit": { "type": "integer", "description": "Most memories to return." }
                }
            }),
            MemoryOp::Remember => json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Short, specific key, e.g. \"owner/coffee-order\"." },
                    "value": { "type": "string", "description": "The fact, in the owner's words." },
                    "layer": { "type": "string", "description": "tacit (preferences, the default), project (ongoing work), entity (people, places, things), or a topic your employee declares." },
                    "namespace": { "type": "string", "description": "Exact namespace; overrides layer. \"tacit/preferences\" facts reach every conversation." }
                },
                "required": ["key", "value"]
            }),
            MemoryOp::Forget => json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "The key of the fact to delete." },
                    "namespace": { "type": "string", "description": "Namespace it is in (default tacit/general)." }
                },
                "required": ["key"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            MemoryOp::Recall => "search remembered facts about the owner",
            MemoryOp::Remember => "save a fact across conversations",
            MemoryOp::Forget => "delete a remembered fact",
        }
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, _input: &Value) -> bool {
        self.op == MemoryOp::Recall
    }

    /// The employee's own memory is its own work.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let blank = |k: &str| input[k].as_str().is_none_or(|s| s.trim().is_empty());
        match self.op {
            MemoryOp::Remember if blank("key") || blank("value") => Err(
                "key and value can't be empty: remember(key: \"owner/name\", value: \"Alice\")"
                    .to_string(),
            ),
            MemoryOp::Forget if blank("key") => Err("key can't be empty.".to_string()),
            _ => Ok(()),
        }
    }

    fn activity(&self, input: &Value) -> String {
        let key = input["key"].as_str().unwrap_or("");
        match self.op {
            MemoryOp::Recall => "checking memory".to_string(),
            MemoryOp::Remember => format!("remembering {key}").trim_end().to_string(),
            MemoryOp::Forget => format!("forgetting {key}").trim_end().to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        let key = input["key"].as_str().unwrap_or("");
        match self.op {
            MemoryOp::Recall => "Checked memory".to_string(),
            MemoryOp::Remember => format!("Remembered {key}").trim_end().to_string(),
            MemoryOp::Forget => format!("Forgot {key}").trim_end().to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                MemoryOp::Recall => self.memory.recall(&input, ctx).await,
                MemoryOp::Remember => self.memory.remember(&input, ctx).await,
                MemoryOp::Forget => self.memory.forget(&input, ctx),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every (namespace, key, user_id) it was asked to embed.
    struct RecordingEmbedder {
        calls: Mutex<Vec<(String, String, String)>>,
    }

    impl MemoryEmbedder for RecordingEmbedder {
        fn embed(&self, namespace: &str, key: &str, user_id: &str) {
            self.calls.lock().unwrap().push((
                namespace.to_string(),
                key.to_string(),
                user_id.to_string(),
            ));
        }
    }

    /// Records every (service, account, secret) write and can be told to
    /// fail — explicit-store routing must never touch the real keychain from
    /// tests.
    struct RecordingKeychain {
        calls: Mutex<Vec<(String, String, String)>>,
        fail: bool,
    }

    impl KeychainStore for RecordingKeychain {
        fn store<'a>(
            &'a self,
            service: &'a str,
            account: &'a str,
            secret: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.lock().unwrap().push((
                    service.to_string(),
                    account.to_string(),
                    secret.to_string(),
                ));
                if self.fail {
                    Err("keychain locked".to_string())
                } else {
                    Ok(())
                }
            })
        }
    }

    struct Rig {
        recall: Box<dyn DynTool>,
        remember: Box<dyn DynTool>,
        forget: Box<dyn DynTool>,
        embedder: Arc<RecordingEmbedder>,
        keychain: Arc<RecordingKeychain>,
        store: Arc<Store>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        /// A file store: each `:memory:` pool connection would be its own
        /// database, and a save is verified on a second connection.
        fn new(keychain_fails: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::new(&dir.path().join("m.db").to_string_lossy()).unwrap());
            let embedder = Arc::new(RecordingEmbedder {
                calls: Mutex::new(Vec::new()),
            });
            let keychain = Arc::new(RecordingKeychain {
                calls: Mutex::new(Vec::new()),
                fail: keychain_fails,
            });
            let mut tools = Memory::new(store.clone(), None, Some(embedder.clone()))
                .with_keychain(keychain.clone())
                .tools()
                .into_iter();
            let (recall, remember, forget) = (
                tools.next().unwrap(),
                tools.next().unwrap(),
                tools.next().unwrap(),
            );
            Self {
                recall,
                remember,
                forget,
                embedder,
                keychain,
                store,
                _dir: dir,
            }
        }
    }

    fn ctx_for(user_id: &str) -> ToolContext {
        ToolContext {
            user_id: user_id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn the_three_tools_are_named_and_always_loaded() {
        let rig = Rig::new(false);
        assert_eq!(
            [rig.recall.name(), rig.remember.name(), rig.forget.name()],
            ["recall", "remember", "forget"]
        );
        assert!(
            !rig.recall.should_defer()
                && !rig.remember.should_defer()
                && !rig.forget.should_defer()
        );
        assert!(rig.recall.read_only(&json!({})) && !rig.remember.read_only(&json!({})));
    }

    #[tokio::test]
    async fn a_save_is_embedded_and_recalled_by_its_key() {
        let rig = Rig::new(false);
        let ctx = ctx_for("user-1");
        let saved = rig
            .remember
            .execute_dyn(&ctx, json!({"key": "person/alice", "value": "Alice is the lead engineer on the migration project."}))
            .await;
        assert!(!saved.is_error, "{}", saved.content);
        assert_eq!(
            rig.embedder.calls.lock().unwrap().as_slice(),
            &[(
                "tacit/general".to_string(),
                "person/alice".to_string(),
                "user-1".to_string()
            )]
        );
        let got = rig
            .recall
            .execute_dyn(&ctx, json!({"query": "person/alice"}))
            .await;
        assert!(got.content.contains("lead engineer"), "{}", got.content);
        let listed = rig.recall.execute_dyn(&ctx, json!({})).await;
        assert!(
            listed.content.contains("person/alice"),
            "no query lists: {}",
            listed.content
        );
        let gone = rig
            .forget
            .execute_dyn(&ctx, json!({"key": "person/alice"}))
            .await;
        assert!(gone.content.starts_with("Forgot 1"), "{}", gone.content);
    }

    /// A query that is no key searches instead.
    #[tokio::test]
    async fn words_that_are_no_key_are_a_search() {
        let rig = Rig::new(false);
        let ctx = ctx_for("user-1");
        rig.store
            .upsert_memory(
                "tacit/general",
                "owner/coffee",
                "Oat flat white, no sugar",
                None,
                None,
                "user-1",
            )
            .unwrap();
        let got = rig
            .recall
            .execute_dyn(&ctx, json!({"query": "flat white"}))
            .await;
        assert!(got.content.contains("owner/coffee"), "{}", got.content);
    }

    /// A credential goes to the keychain (service nebo-memory, account =
    /// "{scope}/{key}") and the row keeps a pointer — never the plaintext.
    #[tokio::test]
    async fn a_credential_goes_to_the_keychain_and_the_row_keeps_a_pointer() {
        let rig = Rig::new(false);
        let ctx = ctx_for("user-1");
        let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
        let saved = rig
            .remember
            .execute_dyn(&ctx, json!({"key": "openai/api-key", "value": secret}))
            .await;
        assert!(
            !saved.is_error && saved.content.contains("keychain"),
            "{}",
            saved.content
        );
        assert_eq!(
            rig.keychain.calls.lock().unwrap().as_slice(),
            &[(
                "nebo-memory".to_string(),
                "user-1/openai/api-key".to_string(),
                secret.to_string()
            )]
        );
        let row = rig
            .store
            .get_memory_by_key_and_user("tacit/general", "openai/api-key", "user-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            row.value,
            "(stored in system keychain: nebo-memory, account user-1/openai/api-key)"
        );
        let got = rig
            .recall
            .execute_dyn(&ctx, json!({"query": "openai/api-key"}))
            .await;
        assert!(got.content.contains("stored in system keychain") && !got.content.contains(secret));
    }

    /// A failed keychain write refuses the save — never a plaintext fallback.
    #[tokio::test]
    async fn a_failed_keychain_write_refuses_the_save() {
        let rig = Rig::new(true);
        let saved = rig
            .remember
            .execute_dyn(
                &ctx_for("user-1"),
                json!({"key": "openai/api-key", "value": "sk-abcdefghijklmnopqrstuvwxyz123456"}),
            )
            .await;
        assert!(
            saved.is_error && saved.content.contains("keychain locked"),
            "{}",
            saved.content
        );
        assert_eq!(rig.store.count_memories().unwrap(), 0);
        assert!(rig.embedder.calls.lock().unwrap().is_empty());
    }

    /// An access code ("4417-echo-9") is the owner's fact, not a credential.
    #[tokio::test]
    async fn an_access_code_is_saved_as_written() {
        let rig = Rig::new(false);
        let value = "The wine cellar access code is 4417-echo-9.";
        let saved = rig
            .remember
            .execute_dyn(
                &ctx_for("user-1"),
                json!({"key": "wine-cellar/access-code", "value": value}),
            )
            .await;
        assert!(!saved.is_error, "{}", saved.content);
        assert!(rig.keychain.calls.lock().unwrap().is_empty());
        let row = rig
            .store
            .get_memory_by_key_and_user("tacit/general", "wine-cellar/access-code", "user-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.value, value);
    }

    /// An isolated employee's save lands under its own context scope only.
    #[tokio::test]
    async fn an_isolated_save_stays_in_its_context() {
        let rig = Rig::new(false);
        let scope = "local:agent:a1:ctx:chat-A";
        let saved = rig
            .remember
            .execute_dyn(&ctx_for(scope), json!({"key": "case/deadline", "value": "The filing deadline for the Smith matter is March 3, 2027."}))
            .await;
        assert!(!saved.is_error, "{}", saved.content);
        assert!(
            rig.store
                .get_memory_by_key_and_user("tacit/general", "case/deadline", scope)
                .unwrap()
                .is_some()
        );
        for other in ["local:agent:a1:ctx:chat-B", "local:agent:a1", "local"] {
            assert!(
                rig.store
                    .get_memory_by_key_and_user("tacit/general", "case/deadline", other)
                    .unwrap()
                    .is_none(),
                "leaked to {other}"
            );
        }
    }

    /// Writes disabled: remember and forget refuse; recall still reads.
    #[tokio::test]
    async fn disabled_writes_refuse_changes_and_keep_reads() {
        let rig = Rig::new(false);
        let ctx = ToolContext {
            user_id: "local:agent:a1".into(),
            memory_writes_disabled: true,
            ..Default::default()
        };
        let saved = rig
            .remember
            .execute_dyn(
                &ctx,
                json!({"key": "case/x", "value": "The Smith matter closes in March."}),
            )
            .await;
        let forgot = rig.forget.execute_dyn(&ctx, json!({"key": "case/x"})).await;
        assert!(saved.is_error && forgot.is_error);
        assert_eq!(rig.store.count_memories().unwrap(), 0);
        rig.store
            .upsert_memory("tacit/general", "owner-fact", "v", None, None, "local")
            .unwrap();
        let got = rig
            .recall
            .execute_dyn(&ctx, json!({"query": "owner-fact"}))
            .await;
        assert!(
            !got.is_error && got.content.contains("v"),
            "{}",
            got.content
        );
    }

    /// Provenance write bar: a barred run is refused; a clean one saves with
    /// its taint in the metadata annex.
    #[tokio::test]
    async fn the_write_bar_refuses_tainted_saves() {
        use types::provenance::ProvenanceClass;
        let rig = Rig::new(false);
        let input = json!({"key": "case/caller-claim", "value": "The caller says the Smith settlement was already wired on Tuesday."});
        let bar = vec![ProvenanceClass::Channel, ProvenanceClass::Phone];
        let barred = ToolContext {
            user_id: "u".into(),
            run_taint: vec![ProvenanceClass::Phone],
            memory_write_bar: bar.clone(),
            ..Default::default()
        };
        let refused = rig.remember.execute_dyn(&barred, input.clone()).await;
        assert!(
            refused.is_error && refused.content.contains("phone calls"),
            "{}",
            refused.content
        );
        let clean = ToolContext {
            user_id: "u".into(),
            run_taint: vec![ProvenanceClass::Web],
            memory_write_bar: bar,
            ..Default::default()
        };
        assert!(!rig.remember.execute_dyn(&clean, input).await.is_error);
        let mem = rig
            .store
            .get_memory_by_key_and_user("tacit/general", "case/caller-claim", "u")
            .unwrap()
            .unwrap();
        assert!(
            mem.metadata.as_deref().unwrap_or("").contains("\"web\""),
            "{:?}",
            mem.metadata
        );
    }

    /// Replying to a coworker without access: no lookups; its own saves work.
    #[tokio::test]
    async fn an_ungranted_audience_gets_no_lookups() {
        let rig = Rig::new(false);
        rig.store
            .upsert_memory(
                "project/case",
                "smith/wire",
                "wired Tuesday",
                None,
                None,
                "local:agent:a1",
            )
            .unwrap();
        let ctx = ToolContext {
            user_id: "local:agent:a1".into(),
            audience_restricted: true,
            ..Default::default()
        };
        for input in [
            json!({"query": "smith/wire"}),
            json!({"query": "wire"}),
            json!({}),
        ] {
            let got = rig.recall.execute_dyn(&ctx, input).await;
            assert!(
                got.is_error && got.content.contains("isn't shared with their role"),
                "{}",
                got.content
            );
        }
        let saved = rig
            .remember
            .execute_dyn(&ctx, json!({"key": "colleague/asked", "value": "The receptionist asked about the Smith wire status today."}))
            .await;
        assert!(!saved.is_error, "{}", saved.content);
    }

    /// Lookups by key reach ancestor scopes, never siblings.
    #[tokio::test]
    async fn a_key_lookup_never_crosses_sibling_scopes() {
        let rig = Rig::new(false);
        rig.store
            .upsert_memory(
                "tacit/general",
                "case/deadline",
                "Case B deadline",
                None,
                None,
                "local:agent:a1:ctx:chat-B",
            )
            .unwrap();
        rig.store
            .upsert_memory(
                "tacit/general",
                "other-agent",
                "v",
                None,
                None,
                "local:agent:a2",
            )
            .unwrap();
        rig.store
            .upsert_memory(
                "tacit/general",
                "owner-fact",
                "owner value",
                None,
                None,
                "local",
            )
            .unwrap();
        let memory = Memory::new(rig.store.clone(), None, None);
        let ctx = ctx_for("local:agent:a1:ctx:chat-A");
        assert_eq!(
            memory
                .find_by_key("tacit/general", "case/deadline", &ctx)
                .unwrap(),
            None
        );
        assert_eq!(
            memory
                .find_by_key("tacit/general", "other-agent", &ctx)
                .unwrap(),
            None
        );
        let owner = memory
            .find_by_key("tacit/general", "owner-fact", &ctx)
            .unwrap()
            .unwrap();
        assert!(owner.contains("owner value"), "{owner}");
    }

    #[test]
    fn ancestors_are_colon_bounded_prefixes() {
        assert!(scope_is_ancestor("local", "local"));
        assert!(scope_is_ancestor(
            "local:agent:a1:ctx:chat-A",
            "local:agent:a1"
        ));
        assert!(scope_is_ancestor("local:agent:a1:ctx:chat-A", "local"));
        assert!(!scope_is_ancestor(
            "local:agent:a1:ctx:chat-A",
            "local:agent:a1:ctx:chat-B"
        ));
        assert!(!scope_is_ancestor("local:agent:a1", "local:agent:a2"));
        assert!(!scope_is_ancestor("local", "local:agent:a1"));
        assert!(!scope_is_ancestor("localx:agent:a1", "local"));
    }
}
