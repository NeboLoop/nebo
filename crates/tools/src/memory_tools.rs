//! `recall`, `remember` and `forget`: the employee's memory. Always loaded,
//! since memory is used on almost every turn. Isolation, the provenance write
//! bar, recall-for-audience and credential routing are the store's rules and
//! hold for every call.
//!
//! Two memories, one store, told apart by the scope (`user_id`) a row is
//! filed under:
//! - LOCAL memory — the owner's scope (`<owner>`), shared by every employee
//!   on this Nebo. "Company memory", "shared", "for everyone" all mean this.
//! - PRIVATE memory — the employee's own scope (`<owner>:agent:<id>`).
//!
//! A conversation-bound scope sits under the private one, decided by the
//! employee's memory mode (`agent::memory::resolve_memory_scope`):
//! - SEALED (`<owner>:agent:<id>:ctx:<ctx>`) — a Separate employee's
//!   conversation with someone other than the owner. It also reads the
//!   employee's private memory.
//! - CONFIDENTIAL (`<owner>:agent:<id>:matter:<ctx>`) — any conversation of a
//!   Confidential employee, the owner's own included. It reads only itself
//!   and local memory, and writes local memory only when the owner's own
//!   words ask for something to be kept for everyone.

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

/// The segment a Separate employee's conversation with someone else hangs
/// off the private scope by.
const SEALED_SEGMENT: &str = ":ctx:";
/// The segment a Confidential conversation hangs off the private scope by.
const CONFIDENTIAL_SEGMENT: &str = ":matter:";

/// What a Confidential conversation's `scope: "local"` write is told when it
/// was made in the conversation because the owner did not ask to share.
const KEPT_CONFIDENTIAL: &str = "Local memory was not touched: this employee keeps every conversation \
     confidential, and local memory changes only when the owner asks for something to be kept for \
     everyone. This conversation's confidential memory was used instead.";

/// A scope bound to one conversation, read back from its `user_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationScope<'a> {
    /// The employee's private scope it sits under.
    pub private: &'a str,
    /// The conversation. Empty for a Confidential run no conversation could
    /// be derived for (its writes are refused).
    pub conversation: &'a str,
    /// A Confidential conversation: it never reads the private scope.
    pub confidential: bool,
}

/// The conversation a scope is bound to, if any — the ONE parser of the
/// conversation segments.
pub fn conversation_scope(user_id: &str) -> Option<ConversationScope<'_>> {
    let sealed = user_id.find(SEALED_SEGMENT).map(|i| (i, SEALED_SEGMENT, false));
    let confidential = user_id.find(CONFIDENTIAL_SEGMENT).map(|i| (i, CONFIDENTIAL_SEGMENT, true));
    let (at, segment, confidential) = match (sealed, confidential) {
        (Some(s), Some(c)) => {
            if s.0 < c.0 {
                s
            } else {
                c
            }
        }
        (s, c) => s.or(c)?,
    };
    Some(ConversationScope {
        private: &user_id[..at],
        conversation: &user_id[at + segment.len()..],
        confidential,
    })
}

/// The scope of `conversation` under the private scope `private` — the ONE
/// writer of the conversation segments (see [`conversation_scope`]).
pub fn conversation_scope_id(private: &str, conversation: &str, confidential: bool) -> String {
    let segment = if confidential {
        CONFIDENTIAL_SEGMENT
    } else {
        SEALED_SEGMENT
    };
    format!("{private}{segment}{conversation}")
}

/// Which memory a scope (`user_id`) is — the ONE reading of the scope
/// convention for the words the model and the owner see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScopeKind {
    /// Shared by every employee on this Nebo.
    Local,
    /// The employee's own.
    Private,
    /// One conversation with someone other than the owner.
    Sealed,
    /// One conversation of a Confidential employee.
    Confidential,
}

impl MemoryScopeKind {
    pub fn of(user_id: &str) -> Self {
        match conversation_scope(user_id) {
            Some(c) if c.confidential => MemoryScopeKind::Confidential,
            Some(_) => MemoryScopeKind::Sealed,
            None if user_id.contains(":agent:") => MemoryScopeKind::Private,
            None => MemoryScopeKind::Local,
        }
    }

    /// How a recalled fact names where it lives.
    pub fn label(self) -> &'static str {
        match self {
            MemoryScopeKind::Local => "local memory",
            MemoryScopeKind::Private => "private memory",
            MemoryScopeKind::Sealed => "this conversation's sealed memory",
            MemoryScopeKind::Confidential => "this conversation's confidential memory",
        }
    }

    /// How a save names where it went, and who can see it.
    fn saved_to(self) -> &'static str {
        match self {
            MemoryScopeKind::Local => "local memory (every employee on this Nebo can find it)",
            MemoryScopeKind::Private => "your private memory (only you can see it)",
            MemoryScopeKind::Sealed => "this conversation's sealed memory (only this conversation can see it)",
            MemoryScopeKind::Confidential => {
                "this conversation's confidential memory (no other conversation can see it)"
            }
        }
    }

    /// Where a `remember` with no `scope: "local"` goes, as the prompt says it.
    pub fn default_save(self) -> &'static str {
        match self {
            MemoryScopeKind::Local => "local memory",
            MemoryScopeKind::Private => "your private memory",
            MemoryScopeKind::Sealed => "this conversation's sealed memory",
            MemoryScopeKind::Confidential => {
                "this conversation's confidential memory, which no other conversation can see"
            }
        }
    }
}

/// Local memory's scope for any resolved scope: the owner part, which every
/// employee on this Nebo reads.
pub fn local_memory_scope(user_id: &str) -> &str {
    user_id.split_once(":agent:").map_or(user_id, |(owner, _)| owner)
}

/// The READ scope chain for a memory user_id — the ONE place ancestor scopes
/// are derived. An employee (and a sealed conversation under it) also reads
/// the scopes above it: `owner:agent:X:ctx:Y` → itself, `owner:agent:X`,
/// `owner` (local memory); the bare owner reads only itself. A Confidential
/// conversation skips the private scope: `owner:agent:X:matter:Y` → itself,
/// `owner`. Sibling employees and sibling conversations are never in it.
pub fn memory_scope_chain(user_id: &str) -> Vec<String> {
    let mut chain = vec![user_id.to_string()];
    if let Some(c) = conversation_scope(user_id)
        && !c.confidential
    {
        chain.push(c.private.to_string());
    }
    chain.push(local_memory_scope(user_id).to_string());
    chain.dedup();
    chain
}

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

    /// The scope a `remember`/`forget` call writes: the run's own scope, or
    /// local memory when the call names `scope: "local"`. Local memory is
    /// read by every employee on this Nebo, so only the owner's own request
    /// (his message in his own chat, or his own call) puts something there —
    /// an unattended run, a caller or a coworker cannot publish to everyone.
    /// The refusal says to tell the owner, never to save it elsewhere: told
    /// to save it privately, the owner's assistant did so on his call and
    /// told him Nebo blocks shared memory (live 2026-09-28).
    ///
    /// A Confidential conversation is sealed: the call's `scope: "local"`
    /// alone never takes a fact out of it. Only the owner's own words asking
    /// for it to be kept for everyone do (`ToolContext::owner_shares`, the
    /// turn's save decision); otherwise the call writes this conversation's
    /// confidential memory, and the second value is the words that say so.
    fn write_scope<'c>(
        input: &Value,
        ctx: &'c ToolContext,
    ) -> Result<(&'c str, Option<&'static str>), ToolResult> {
        if input["scope"].as_str() != Some("local") {
            return Ok((&ctx.user_id, None));
        }
        if MemoryScopeKind::of(&ctx.user_id) == MemoryScopeKind::Confidential
            && !(ctx.owner_request && ctx.owner_shares)
        {
            return Ok((&ctx.user_id, Some(KEPT_CONFIDENTIAL)));
        }
        if !ctx.owner_request {
            return Err(ToolResult::error(
                "Not saved to local memory: local memory is shared by every employee on this \
                 Nebo, so only the owner's own request (their message in their own \
                 conversation, or their own call) changes it, and this run was not started by \
                 one. Nothing was saved. Tell the owner it was not saved and why; don't save it \
                 anywhere else in its place.",
            ));
        }
        Ok((local_memory_scope(&ctx.user_id), None))
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

        let (scope, kept_here) = match Self::write_scope(input, ctx) {
            Ok(written) => written,
            Err(refused) => return refused,
        };
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
                        &format!("{}/{}", scope, key),
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
                        scope
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

        debug!(namespace, key, value_len = stored_value.len(), scope, "memory store attempt");

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
            scope,
        ) {
            return ToolResult::error(format!(
                "Failed to save memory [{namespace}] {key}: {e}. Do not retry immediately — this \
                 is a database error, not a parameter issue."
            ));
        }
        // Verify the write on a different pool connection.
        match self
            .store
            .get_memory_by_key_and_user(namespace, key, scope)
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                let total = self.store.count_memories().ok();
                warn!(namespace, key, scope, total_memories = total.unwrap_or(-1),
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
            embedder.embed(namespace, key, scope);
        }
        let saved = match keychain_kind {
            Some(kind) => format!(
                "Saved a pointer for {key} in [{namespace}] of {}; the value was credential-shaped \
                 ({kind}), and the secret itself is in the OS keychain (service \
                 {MEMORY_KEYCHAIN_SERVICE}, account {}/{key}). Tell the owner where it lives.",
                MemoryScopeKind::of(scope).saved_to(),
                scope
            ),
            None => format!(
                "Saved to {}: [{namespace}] {key} = {stored_value}",
                MemoryScopeKind::of(scope).saved_to()
            ),
        };
        ToolResult::ok(with_why(saved, kept_here))
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

    /// The fact stored under `key`: in `namespace` first, then in any
    /// namespace — each time in the run's own scope before the scopes above
    /// it (private memory, then local memory). Sibling employees and sibling
    /// conversations are never readable.
    fn find_by_key(
        &self,
        namespace: &str,
        key: &str,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        let chain = memory_scope_chain(&ctx.user_id);
        for scope in &chain {
            match self.store.get_memory_by_key_and_user(namespace, key, scope) {
                Ok(Some(mem)) => {
                    let _ = self
                        .store
                        .increment_memory_access_by_key(namespace, key, scope);
                    return Ok(Some(format!(
                        "[{}] {}: {} ({})",
                        mem.namespace,
                        mem.key,
                        mem.value,
                        MemoryScopeKind::of(scope).label()
                    )));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(format!(
                        "Failed to recall memory [{namespace}] {key}: {e}. Do not retry — this is \
                         a database error."
                    ));
                }
            }
        }
        // Key-only lookup across namespaces, same scope order.
        for scope in &chain {
            if let Some(m) = self.store.find_memory_by_key(key, scope).ok().flatten() {
                let _ = self
                    .store
                    .increment_memory_access_by_key(&m.namespace, key, scope);
                return Ok(Some(format!(
                    "[{}] {}: {} (found in namespace {}; {})",
                    m.namespace,
                    m.key,
                    m.value,
                    m.namespace,
                    MemoryScopeKind::of(scope).label()
                )));
            }
        }
        Ok(None)
    }

    async fn search(&self, query: &str, limit: usize, ctx: &ToolContext) -> ToolResult {
        // Hybrid search (FTS5 + vector) when available — it reads the whole
        // scope chain.
        if let Some(ref searcher) = self.hybrid_searcher {
            let results = searcher.search(query, &ctx.user_id, limit, None).await;
            if !results.is_empty() {
                let lines: Vec<String> = results
                    .iter()
                    .map(|r| {
                        format!(
                            "- [{}] {}: {} ({}, relevance {:.2}/1)",
                            r.namespace,
                            r.key,
                            r.value,
                            MemoryScopeKind::of(&r.scope).label(),
                            r.score
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
        let mut lines: Vec<String> = Vec::new();
        for scope in memory_scope_chain(&ctx.user_id) {
            match self
                .store
                .search_memories_by_user(&scope, query, limit as i64, 0)
            {
                Ok(memories) => lines.extend(memories.iter().map(|m| {
                    format!(
                        "- [{}] {}: {} ({})",
                        m.namespace,
                        m.key,
                        m.value,
                        MemoryScopeKind::of(&scope).label()
                    )
                })),
                Err(e) => {
                    return ToolResult::error(format!(
                        "Memory search failed: {e}. Do not retry — this is a database error. \
                         Call recall with no query to list memories instead."
                    ));
                }
            }
        }
        lines.truncate(limit);
        if lines.is_empty() {
            return ToolResult::ok(format!(
                "No memories found matching: {query} (searched {}).",
                searched(&ctx.user_id)
            ));
        }
        ToolResult::ok(format!(
            "Found {} memories (text match):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    /// This employee's memories and local memory — never another employee's.
    fn list(&self, prefix: &str, limit: i64, ctx: &ToolContext) -> ToolResult {
        let mut lines: Vec<String> = Vec::new();
        for scope in memory_scope_chain(&ctx.user_id) {
            let room = limit - lines.len() as i64;
            if room <= 0 {
                break;
            }
            match self
                .store
                .list_memories_by_user_and_namespace(&scope, prefix, room, 0)
            {
                Ok(mems) => lines.extend(mems.iter().map(|m| {
                    format!(
                        "- [{}] {}: {} ({})",
                        m.namespace,
                        m.key,
                        m.value,
                        MemoryScopeKind::of(&scope).label()
                    )
                })),
                Err(e) => return ToolResult::error(format!("Failed to list memories: {e}")),
            }
        }
        if lines.is_empty() {
            return ToolResult::ok(format!(
                "No memories in namespace prefix '{prefix}' ({}). (With no namespace this lists \
                 tacit/; pass namespace: \"project\" or \"entity/\" to see others.)",
                searched(&ctx.user_id)
            ));
        }
        let page_note = if lines.len() as i64 >= limit {
            format!(" (first {limit}; raise limit for more)")
        } else {
            String::new()
        };
        ToolResult::ok(format!(
            "{} memories in {prefix}{page_note}:\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }

    fn forget(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        if let Some(refused) = Self::writes_refused(ctx) {
            return refused;
        }
        let (scope, kept_here) = match Self::write_scope(input, ctx) {
            Ok(written) => written,
            Err(refused) => return refused,
        };
        let key = input["key"].as_str().unwrap_or("");
        let namespace = input["namespace"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or(DEFAULT_NAMESPACE);
        let place = MemoryScopeKind::of(scope).label();
        match self
            .store
            .delete_memory_by_key_and_user(namespace, key, scope)
        {
            Ok(n) if n > 0 => ToolResult::ok(with_why(
                format!("Forgot {n} entries for key '{key}' in {namespace} of {place}."),
                kept_here,
            )),
            Ok(_) => ToolResult::ok(with_why(
                format!(
                    "Nothing forgotten: no memory with key '{key}' in {namespace} of {place}. \
                     recall with the key shows the namespace and the memory it is in."
                ),
                kept_here,
            )),
            Err(e) => ToolResult::error(format!("Failed to forget: {e}")),
        }
    }
}

/// A write's answer, with the reason on its own line when the write stayed
/// somewhere other than the memory the call named.
fn with_why(said: String, why: Option<&str>) -> String {
    match why {
        Some(why) => format!("{said}\n{why}"),
        None => said,
    }
}

/// The memories a read covered, in words: "private memory and local memory".
fn searched(user_id: &str) -> String {
    memory_scope_chain(user_id)
        .iter()
        .map(|s| MemoryScopeKind::of(s).label())
        .collect::<Vec<_>>()
        .join(" and ")
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
                 - `query` can be a saved key or words to search for.\n\
                 - Leave `query` empty to list recent memories; `namespace` narrows the list (\"project\", \"entity/\").\n\
                 - A fact that reads \"(stored in system keychain: …)\" is a pointer: fetch the secret only when the owner asks for it."
                .to_string(),
            MemoryOp::Remember => "Saves a fact worth keeping across conversations, in the owner's exact words. Use a short, specific key (\"owner/coffee-order\").\n\
                 - `scope` \"local\" (company memory, shared, for everyone) is read by every employee on this Nebo; the default is your private memory.\n\
                 - When the owner asks you to save something, save it before saying so: the ask is their consent. Passwords and API keys go to the system keychain and the memory keeps a pointer.\n\
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
                    "namespace": { "type": "string", "description": "Exact namespace; overrides layer." },
                    "scope": { "type": "string", "enum": ["private", "local"] }
                },
                "required": ["key", "value"]
            }),
            MemoryOp::Forget => json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "The key of the fact to delete." },
                    "namespace": { "type": "string", "description": "Namespace it is in (default tacit/general)." },
                    "scope": { "type": "string", "enum": ["private", "local"] }
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
        if self.op != MemoryOp::Recall
            && !matches!(input.get("scope"), None | Some(Value::Null))
            && !matches!(input["scope"].as_str(), Some("private" | "local"))
        {
            return Err(
                "scope is \"private\" (your own memory) or \"local\" (shared by every employee \
                 on this Nebo)."
                    .to_string(),
            );
        }
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

    fn employee(owner: &str, id: &str, owner_request: bool) -> ToolContext {
        ToolContext {
            user_id: format!("{owner}:agent:{id}"),
            owner_request,
            ..Default::default()
        }
    }

    /// Local memory is read by every employee on this Nebo; a private fact
    /// is read by its employee alone. The save says which one it was.
    #[tokio::test]
    async fn a_local_save_reaches_every_employee_and_a_private_one_stays_private() {
        let rig = Rig::new(false);
        let (a, b, c) = (employee("o", "a", true), employee("o", "b", false), employee("o", "c", true));
        let recipe = "Sheet-pan lemon chickpeas: 2 cans chickpeas, 2 tbsp olive oil, zest of one lemon, roast at 425F for 25 minutes.";
        let saved = rig
            .remember
            .execute_dyn(&a, json!({"key": "recipes/lemon-chickpeas", "value": recipe, "layer": "project", "scope": "local"}))
            .await;
        assert!(!saved.is_error && saved.content.starts_with("Saved to local memory"), "{}", saved.content);
        assert!(rig.store.get_memory_by_key_and_user("project", "recipes/lemon-chickpeas", "o").unwrap().is_some());

        // Another employee, and a sealed conversation of it, find it by
        // words and by key, and are told it is in local memory.
        for reader in [b.clone(), ctx_for("o:agent:b:ctx:caller-1")] {
            let found = rig.recall.execute_dyn(&reader, json!({"query": "chickpeas"})).await;
            assert!(found.content.contains("425F") && found.content.contains("(local memory)"), "{}", found.content);
            let by_key = rig.recall.execute_dyn(&reader, json!({"query": "recipes/lemon-chickpeas"})).await;
            assert!(by_key.content.contains("425F") && by_key.content.contains("local memory"), "{}", by_key.content);
            let listed = rig.recall.execute_dyn(&reader, json!({"namespace": "project"})).await;
            assert!(listed.content.contains("recipes/lemon-chickpeas"), "{}", listed.content);
        }

        // A private save is its employee's alone, and says so.
        let private = rig
            .remember
            .execute_dyn(&c, json!({"key": "owner/gate-code-hint", "value": "The owner keeps the side gate code on the fridge calendar."}))
            .await;
        assert!(private.content.starts_with("Saved to your private memory"), "{}", private.content);
        assert!(!private.content.contains("local memory"), "{}", private.content);
        for query in ["owner/gate-code-hint", "fridge calendar"] {
            let other = rig.recall.execute_dyn(&b, json!({"query": query})).await;
            assert!(!other.content.contains("side gate"), "{query}: {}", other.content);
        }
        let own = rig.recall.execute_dyn(&c, json!({"query": "fridge calendar"})).await;
        assert!(own.content.contains("(private memory)"), "{}", own.content);
    }

    /// Only the owner's own message changes local memory: a coworker, a
    /// caller or an unattended run is refused, and nothing is written.
    #[tokio::test]
    async fn local_memory_changes_only_at_the_owners_request() {
        let rig = Rig::new(false);
        let unattended = employee("o", "a", false);
        let refused = rig
            .remember
            .execute_dyn(&unattended, json!({"key": "team/standup", "value": "The team standup moved to Thursdays at nine.", "scope": "local"}))
            .await;
        assert!(refused.is_error && refused.content.starts_with("Not saved to local memory"), "{}", refused.content);
        // Told to tell the owner, never to save it somewhere else in its place
        // (live 2026-09-28: told to save it privately, it did, and told the
        // owner Nebo blocks shared memory).
        assert!(refused.content.contains("Tell the owner it was not saved and why"), "{}", refused.content);
        assert!(!refused.content.contains("private"), "{}", refused.content);
        assert_eq!(rig.store.count_memories().unwrap(), 0);

        rig.store.upsert_memory("tacit/general", "team/standup", "Thursdays", None, None, "o").unwrap();
        let kept = rig.forget.execute_dyn(&unattended, json!({"key": "team/standup", "scope": "local"})).await;
        assert!(kept.is_error, "{}", kept.content);
        let gone = rig.forget.execute_dyn(&employee("o", "a", true), json!({"key": "team/standup", "scope": "local"})).await;
        assert!(gone.content.starts_with("Forgot 1") && gone.content.contains("local memory"), "{}", gone.content);
    }

    /// Nothing the memory tools say names a global memory: this Nebo's
    /// memory is local and private, complete without any account.
    #[tokio::test]
    async fn the_memory_tools_never_mention_a_global_memory() {
        let rig = Rig::new(false);
        let ctx = employee("o", "a", true);
        let mut said = Vec::new();
        for tool in [&rig.recall, &rig.remember, &rig.forget] {
            said.push(tool.description());
            said.push(tool.schema().to_string());
        }
        said.push(rig.remember.execute_dyn(&ctx, json!({"key": "k/one", "value": "The owner prefers morning meetings before ten.", "scope": "local"})).await.content);
        said.push(rig.recall.execute_dyn(&ctx, json!({"query": "morning"})).await.content);
        said.push(rig.recall.execute_dyn(&ctx, json!({"query": "nothing-matches-this"})).await.content);
        said.push(rig.recall.execute_dyn(&ctx, json!({})).await.content);
        for text in said {
            assert!(!text.to_lowercase().contains("global"), "{text}");
        }
        assert!(rig.remember.validate_input(&json!({"key": "k", "value": "v", "scope": "global"})).is_err());
        assert!(rig.remember.validate_input(&json!({"key": "k", "value": "v", "scope": "local"})).is_ok());
    }

    /// The read chain: the run's own scope, then private memory, then local
    /// memory — never a sibling employee or a sibling conversation.
    #[test]
    fn the_read_chain_is_own_then_private_then_local() {
        assert_eq!(memory_scope_chain("local"), vec!["local"]);
        assert_eq!(memory_scope_chain("local:agent:a1"), vec!["local:agent:a1", "local"]);
        assert_eq!(
            memory_scope_chain("local:agent:a1:ctx:chat-A"),
            vec!["local:agent:a1:ctx:chat-A", "local:agent:a1", "local"]
        );
        assert_eq!(local_memory_scope("local:agent:a1:ctx:chat-A"), "local");
        assert_eq!(MemoryScopeKind::of("local"), MemoryScopeKind::Local);
        assert_eq!(MemoryScopeKind::of("local:agent:a1"), MemoryScopeKind::Private);
        assert_eq!(MemoryScopeKind::of("local:agent:a1:ctx:c"), MemoryScopeKind::Sealed);
    }

    /// A Confidential conversation reads itself and local memory: never the
    /// employee's private memory, never a sibling conversation. The segments
    /// read back what was written, whichever comes first.
    #[test]
    fn a_confidential_conversation_reads_itself_then_local() {
        let a = conversation_scope_id("local:agent:a1", "chat-A", true);
        assert_eq!(a, "local:agent:a1:matter:chat-A");
        assert_eq!(memory_scope_chain(&a), vec![a.clone(), "local".to_string()]);
        assert_eq!(MemoryScopeKind::of(&a), MemoryScopeKind::Confidential);
        assert_eq!(
            conversation_scope(&a),
            Some(ConversationScope { private: "local:agent:a1", conversation: "chat-A", confidential: true })
        );
        let sealed = conversation_scope_id("local:agent:a1", "dm:matter:9", false);
        assert_eq!(
            conversation_scope(&sealed),
            Some(ConversationScope { private: "local:agent:a1", conversation: "dm:matter:9", confidential: false })
        );
        assert_eq!(conversation_scope("local:agent:a1"), None);
        assert_eq!(local_memory_scope(&a), "local");
    }

    /// In a Confidential conversation a save with no scope stays in that
    /// conversation, and says so; another conversation of the same employee
    /// never finds it by words, key or listing, while a local fact reaches
    /// both. A save the owner asked to put in local memory names local memory.
    #[tokio::test]
    async fn a_confidential_save_stays_in_its_conversation() {
        let rig = Rig::new(false);
        let conv = |c: &str| ToolContext {
            user_id: conversation_scope_id("o:agent:law", c, true),
            owner_request: true,
            owner_shares: true,
            ..Default::default()
        };
        let (a, b) = (conv("client-a"), conv("client-b"));
        let saved = rig
            .remember
            .execute_dyn(&a, json!({"key": "case/settlement", "value": "The Harlow settlement offer is 410,000, ALDER-1.", "layer": "project"}))
            .await;
        assert!(saved.content.starts_with("Saved to this conversation's confidential memory"), "{}", saved.content);
        let private = rig
            .remember
            .execute_dyn(&a, json!({"key": "case/judge", "value": "Judge Okafor hears the Harlow motions, ALDER-2.", "scope": "private"}))
            .await;
        assert!(private.content.starts_with("Saved to this conversation's confidential memory"), "private is this conversation here: {}", private.content);
        assert!(rig.store.get_memory_by_key_and_user("project", "case/settlement", "o:agent:law:matter:client-a").unwrap().is_some());
        assert!(rig.store.get_memory_by_key_and_user("project", "case/settlement", "o:agent:law").unwrap().is_none());

        let local = rig
            .remember
            .execute_dyn(&a, json!({"key": "office/hours", "value": "The office closes at four on Fridays.", "scope": "local"}))
            .await;
        assert!(local.content.starts_with("Saved to local memory"), "{}", local.content);

        for input in [json!({"query": "Harlow settlement"}), json!({"query": "case/settlement"}), json!({"query": "case/judge"}), json!({"namespace": "project"}), json!({})] {
            let seen = rig.recall.execute_dyn(&b, input.clone()).await;
            assert!(!seen.content.contains("ALDER"), "{input}: {}", seen.content);
        }
        let hours = rig.recall.execute_dyn(&b, json!({"query": "Fridays"})).await;
        assert!(hours.content.contains("four on Fridays") && hours.content.contains("(local memory)"), "{}", hours.content);
        let own = rig.recall.execute_dyn(&a, json!({"query": "Harlow settlement"})).await;
        assert!(own.content.contains("ALDER-1") && own.content.contains("confidential memory"), "{}", own.content);
    }

    /// The v0.16.0 proof's leak (m08, 2 of 3 runs): a Confidential
    /// employee's own `scope: "local"` in the owner's conversation, with no
    /// ask from the owner to share, put a client's deposition date in local
    /// memory for every employee. The call's scope alone never takes a fact
    /// out of the conversation: it lands in this conversation's confidential
    /// memory, the answer says so and never says local memory got it, and no
    /// other conversation or employee finds it. A forget stays here too.
    #[tokio::test]
    async fn a_confidential_local_write_the_owner_did_not_ask_to_share_stays_in_the_conversation() {
        let rig = Rig::new(false);
        let pryce = ToolContext {
            user_id: conversation_scope_id("o:agent:law", "client-b", true),
            owner_request: true,
            owner_shares: false,
            ..Default::default()
        };
        let saved = rig
            .remember
            .execute_dyn(&pryce, json!({"key": "cases/pryce/deposition", "value": "The Pryce deposition is on the 14th, BIRCH-1.", "layer": "project", "scope": "local"}))
            .await;
        assert!(!saved.is_error, "{}", saved.content);
        assert!(saved.content.starts_with("Saved to this conversation's confidential memory"), "{}", saved.content);
        assert!(!saved.content.contains("Saved to local memory"), "{}", saved.content);
        assert!(saved.content.contains("Local memory was not touched"), "the answer says why: {}", saved.content);
        let conversation = "o:agent:law:matter:client-b";
        assert!(rig.store.get_memory_by_key_and_user("project", "cases/pryce/deposition", conversation).unwrap().is_some());
        assert!(rig.store.get_memory_by_key_and_user("project", "cases/pryce/deposition", "o").unwrap().is_none(), "nothing in local memory");

        for reader in [ctx_for(&conversation_scope_id("o:agent:law", "client-a", true)), employee("o", "clerk", true)] {
            for input in [json!({"query": "Pryce deposition"}), json!({"query": "cases/pryce/deposition"}), json!({"namespace": "project"})] {
                let seen = rig.recall.execute_dyn(&reader, input.clone()).await;
                assert!(!seen.content.contains("BIRCH-1"), "{} {input}: {}", reader.user_id, seen.content);
            }
        }

        rig.store.upsert_memory("tacit/general", "office/hours", "The office closes at four.", None, None, "o").unwrap();
        let forgot = rig.forget.execute_dyn(&pryce, json!({"key": "office/hours", "scope": "local"})).await;
        assert!(forgot.content.starts_with("Nothing forgotten") && forgot.content.contains("confidential memory"), "{}", forgot.content);
        assert!(rig.store.get_memory_by_key_and_user("tacit/general", "office/hours", "o").unwrap().is_some(), "the local fact is untouched");
    }

    /// m09: when the owner's own words ask for a fact to be kept for
    /// everyone, a Confidential conversation's save goes to local memory,
    /// and every other conversation of the employee and every other employee
    /// finds it. The owner's ask is needed on the owner's own turn: a share
    /// on a turn the owner did not start is kept in the conversation.
    #[tokio::test]
    async fn a_confidential_save_the_owner_asked_to_share_goes_to_local_memory() {
        let rig = Rig::new(false);
        let asked = ToolContext {
            user_id: conversation_scope_id("o:agent:law", "front-desk", true),
            owner_request: true,
            owner_shares: true,
            ..Default::default()
        };
        let saved = rig
            .remember
            .execute_dyn(&asked, json!({"key": "office/friday-close", "value": "The office closes at 4pm on Fridays, CEDAR-1.", "scope": "local"}))
            .await;
        assert!(saved.content.starts_with("Saved to local memory"), "{}", saved.content);
        assert!(!saved.content.contains("not touched"), "{}", saved.content);
        assert!(rig.store.get_memory_by_key_and_user("tacit/general", "office/friday-close", "o").unwrap().is_some());
        for reader in [ctx_for(&conversation_scope_id("o:agent:law", "client-a", true)), employee("o", "clerk", false)] {
            let found = rig.recall.execute_dyn(&reader, json!({"query": "Fridays"})).await;
            assert!(found.content.contains("CEDAR-1") && found.content.contains("(local memory)"), "{}: {}", reader.user_id, found.content);
        }

        let not_the_owners_turn = ToolContext { owner_request: false, ..asked.clone() };
        let kept = rig
            .remember
            .execute_dyn(&not_the_owners_turn, json!({"key": "office/parking", "value": "Visitors park in bay 3, CEDAR-2.", "scope": "local"}))
            .await;
        assert!(kept.content.starts_with("Saved to this conversation's confidential memory"), "{}", kept.content);
        assert!(rig.store.get_memory_by_key_and_user("tacit/general", "office/parking", "o").unwrap().is_none());
    }

    /// Separate conversations: a conversation with someone other than the
    /// owner is sealed, and the owner never speaks in it, so its
    /// `scope: "local"` is refused and nothing is written; the owner's own
    /// conversations share the employee's private memory, and local memory
    /// changes there at the owner's request as for any employee.
    #[tokio::test]
    async fn a_sealed_conversation_never_writes_local_memory() {
        let rig = Rig::new(false);
        let caller = ToolContext { user_id: conversation_scope_id("o:agent:desk", "caller-1", false), ..Default::default() };
        let refused = rig
            .remember
            .execute_dyn(&caller, json!({"key": "team/vendor", "value": "The vendor list moved to the shared drive.", "scope": "local"}))
            .await;
        assert!(refused.is_error && refused.content.starts_with("Not saved to local memory"), "{}", refused.content);
        assert_eq!(rig.store.count_memories().unwrap(), 0);

        let owner = employee("o", "desk", true);
        let saved = rig
            .remember
            .execute_dyn(&owner, json!({"key": "team/vendor", "value": "The vendor list moved to the shared drive.", "scope": "local"}))
            .await;
        assert!(saved.content.starts_with("Saved to local memory"), "{}", saved.content);
    }
}
