use std::sync::Arc;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Trait for advisor deliberation (implemented by agent::advisors::Runner).
/// Defined here to avoid circular dependencies between tools and agent crates.
pub trait AdvisorDeliberator: Send + Sync {
    fn deliberate<'a>(
        &'a self,
        task: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Installs a marketplace code through the ONE canonical server pathway
/// (`server::codes::handle_code`). Defined here (tools crate) and implemented in the
/// server — which owns `AppState` — to avoid a tools→server crate cycle, the same
/// pattern as [`AdvisorDeliberator`]/[`StructuredAgent`]. Routing every install through
/// this single pathway means skills, plugins (binary download + tool/hook
/// re-registration), agents, apps, and collections all install AND cascade correctly —
/// instead of the per-type API shortcuts that bypass the cascade.
pub trait CodeInstaller: Send + Sync {
    /// Install any `PREFIX-XXXX-XXXX` code; returns a human-readable result string.
    fn install<'a>(
        &'a self,
        code: &'a str,
        by: InstalledBy,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>>;
}

/// Whose act an install is. The owner consents to jobs: an employee hired
/// by the owner's own act (a Hire tap, a code the owner pasted in their app,
/// a hire on their account, a call in a run the owner started from their
/// app) holds the job its package declares. Any other install (a code posted
/// in a chat channel, a call in a run the owner didn't start) brings the
/// employee in with no job, so it asks before it acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledBy {
    Owner,
    Other,
}

impl InstalledBy {
    /// Who is behind a tool call's install: the owner when the run is the
    /// owner's own, from their app.
    pub fn of(ctx: &ToolContext) -> Self {
        if ctx.origin == crate::Origin::User && !ctx.audience_restricted {
            InstalledBy::Owner
        } else {
            InstalledBy::Other
        }
    }
}

/// One structured sub-agent request for the deep-research harness: a helper of the research
/// run that may call only the named `aux_tools` and whose final answer is one JSON object
/// matching `schema` (see `agent::structured_agent::StructuredRunner`).
pub struct StructuredTask {
    pub system: String,
    pub task: String,
    pub schema: serde_json::Value,
    /// STRAP tool names the sub-agent may call during its free phase (e.g. `["web"]`).
    pub aux_tools: Vec<String>,
    /// Browser-tab / session identity for this sub-agent. Aux-tool calls execute under
    /// this key so each sub-agent owns its own tab (the 1:1 sub-agent→tab model), while
    /// siblings keyed `subagent:{parent}:sa-{id}` share the parent's visited-page cache.
    pub tab_key: String,
    /// Optional cap on free-phase tool-use turns. `None` → the agent default. Used to
    /// hold a sub-agent to a single tool call (e.g. the reference deep-research search
    /// agent does ONE WebSearch per angle, not an open-ended browse loop).
    pub max_tool_turns: Option<u32>,
}

/// Trait for running forced-structured-output sub-agents AND executing single tools on
/// their behalf (implemented by `agent::structured_agent::StructuredRunner`). Defined
/// here so the deep-research harness in the tools crate can drive sub-agents without a
/// circular dependency on the agent crate (which owns the providers). Both methods
/// dispatch tool calls through the canonical `Registry::execute` — there is no separate
/// web pathway.
pub trait StructuredAgent: Send + Sync {
    /// `activity`, when given, is bumped by the runner on every provider stream
    /// event and completed tool call — the caller's progress signal for
    /// detecting a stalled sub-agent without a wall-clock cap on legit work.
    fn run<'a>(
        &'a self,
        task: StructuredTask,
        activity: Option<Arc<std::sync::atomic::AtomicU64>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'a>,
    >;

    /// Execute one registered tool directly (no LLM) under `tab_key` — for the harness's
    /// deterministic fetch+sanitize step. Returns the canonical [`ToolResult`] (content +
    /// `http_status` + `is_error`) so the caller can branch on rate-limit statuses.
    fn execute_tool<'a>(
        &'a self,
        tab_key: String,
        tool: String,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

    /// Close the browser tab/page this sub-agent opened under `tab_key`, once it has
    /// finished — the 1:1 sub-agent→tab cleanup. No-op if it never opened one.
    fn close_tab<'a>(
        &'a self,
        tab_key: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// Trait for hybrid memory search (implemented by agent::search wrapper).
/// Combines FTS5 text search + vector cosine similarity with adaptive weights.
pub trait HybridSearcher: Send + Sync {
    /// `min_score` overrides the searcher's default relevance floor when
    /// `Some` (the tool uses the default; prompt recall passes `Some(0.0)`
    /// so FTS-only installs still surface their top-ranked matches).
    fn search<'a>(
        &'a self,
        query: &'a str,
        user_id: &'a str,
        limit: usize,
        min_score: Option<f64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HybridSearchResult>> + Send + 'a>>;
}

/// Result from hybrid memory search.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// Backing memory row, when the hit resolves to one (session chunks have
    /// none). Lets prompt recall dedupe against the identity slice and bump
    /// access accounting for injected memories.
    pub memory_id: Option<i64>,
    pub key: String,
    pub value: String,
    pub namespace: String,
    pub score: f64,
}

/// Fire-and-forget embedding hook for explicitly stored memories (implemented
/// by `agent::search_adapter::MemoryEmbedAdapter`). Same adapter-injection
/// pattern as [`HybridSearcher`]: the tools crate cannot depend on the agent
/// crate (which owns the embedding pipeline), so the server injects this at
/// wiring time. Keeps explicit `memory store` writes on the SAME chunk+embed
/// pathway (`agent::memory::embed_memories_async`) as automatic extraction.
pub trait MemoryEmbedder: Send + Sync {
    /// Chunk + embed the memory stored at (namespace, key) for `user_id`.
    /// Must spawn its work in the background — never blocks the tool call.
    fn embed(&self, namespace: &str, key: &str, user_id: &str);
}

/// OS keychain writer for credential routing on explicit memory stores.
/// The default implementation delegates to the ONE os keychain pathway
/// (`keychain_tool::KeychainTool`'s add action) — trait-injected so tests
/// never write to the real system keychain.
pub trait KeychainStore: Send + Sync {
    /// Store `secret` under (service, account). `Err` carries the
    /// human-readable failure from the platform keychain.
    fn store<'a>(
        &'a self,
        service: &'a str,
        account: &'a str,
        secret: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;
}

/// Default [`KeychainStore`] — calls the canonical `KeychainTool` add action
/// (macOS `security`, Linux `secret-tool`, Windows `cmdkey`); no second
/// keychain implementation.
pub(crate) struct OsKeychain;

impl KeychainStore for OsKeychain {
    fn store<'a>(
        &'a self,
        service: &'a str,
        account: &'a str,
        secret: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let result = crate::keychain_tool::KeychainTool::new()
                .execute_dyn(
                    &ToolContext::default(),
                    serde_json::json!({
                        "action": "add",
                        "service": service,
                        "account": account,
                        "password": secret,
                    }),
                )
                .await;
            if result.is_error {
                Err(result.content)
            } else {
                Ok(())
            }
        })
    }
}
