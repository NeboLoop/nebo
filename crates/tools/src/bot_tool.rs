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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>>;
}

/// One structured sub-agent request for the deep-research harness. The agent does free
/// tool work with the named `aux_tools`, then is FORCED through a schema-validated
/// `StructuredOutput` call (see `agent::structured::agent_structured`).
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

/// The `agent` tool: employees, the ones installed here and the ones the
/// marketplace can hire (the registry). Memory, helpers, tasks, sessions,
/// research, the profile and asking the owner are their own tools.
pub struct AgentTool {
    persona: crate::agent_tool::PersonaTool,
}

impl AgentTool {
    pub fn new(persona: crate::agent_tool::PersonaTool) -> Self {
        Self { persona }
    }

    fn action(input: &serde_json::Value) -> &str {
        input.get("action").and_then(|v| v.as_str()).unwrap_or("")
    }
}

impl DynTool for AgentTool {
    fn name(&self) -> &str {
        "agent"
    }

    fn description(&self) -> String {
        let mut description = String::from(
            "Employees: the ones installed here, and the ones you can hire.\n\
         - agent(resource: \"registry\", action: \"discover\", query: [\"bookkeeper\", \"social media manager\"]) — SEARCH THE MARKETPLACE. \
         ONE call for EVERY role the user named (a list; a single string works too): each query comes back with its \
         employees and its tools, ranked, marked [already hired]/[already installed], and ONE hire card offers the best \
         match for all of them with one confirm. Never search roles one at a time. Also takes department, limit, offset; \
         omit query to page the whole catalog.\n\
         With a query or without one, the page shows BOTH catalogs: the employees and the tools, connections \
         and services they use. An employee hires on the card discover offers. ",
        );
        description.push_str(crate::plugin_tool::TOOL_INSTALL_DOOR);
        description.push_str(
            "\n\
         STAFFING: when the user wants to set up a business, add people, or asks who could do a job, the employee is the \
         hire and a tool is what they use — discover shows both. Departments: \
         accounting, sales, customer-support, marketing, direct-response, operations, people-hr, legal, it, analytics, \
         product-engineering, executive, corporate. Results put NeboAI's own employees first (tagged [NeboAI]) — prefer them. \
         Never paste install codes into chat. NEVER say the marketplace has nothing until discover itself says so.\n\
         - agent(resource: \"registry\", action: \"list\") — List installed agents\n\
         - agent(resource: \"registry\", action: \"activate\", name: \"...\") — Activate an agent\n\
         - agent(resource: \"registry\", action: \"info\", name: \"...\") — Show agent details\n\
         - agent(resource: \"registry\", action: \"install\", code: \"AGNT-XXXX-XXXX\") — Install from marketplace\n\
         - agent(resource: \"registry\", action: \"create\", name: \"...\", description: \"...\", automations: [{\"name\": \"...\", \"schedule\": \"weekdays at 9am\", \"steps\": [\"...\"]}]) — Create a user agent. Any recurring duty MUST go in automations (each becomes the agent's own scheduled workflow — shown in its Workflows tab and the Schedule page, runs as the agent) — never a bare create plus event crons.\n\
         - agent(resource: \"registry\", action: \"update\", name: \"...\", add_automations: [...]) — Add workflows to an existing agent (automations: replaces ALL existing ones; remove_automations: delete by name)",
        );
        description
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": { "type": "string", "enum": ["registry"], "description": "Always registry." },
                "action": {
                    "type": "string",
                    "enum": ["discover", "list", "info", "install", "create", "update", "delete", "activate", "deactivate", "setup", "repair", "reload", "stats"],
                    "description": "What to do with employees."
                },
                "query": { "type": ["string", "array"], "items": { "type": "string" }, "description": "discover: one role, or a LIST of roles and tools — one call answers all of them." },
                "limit": { "type": "integer", "description": "Max results" },
                "department": { "type": "string", "description": "discover: narrow the marketplace search to one department (accounting, sales, customer-support, marketing, direct-response, operations, people-hr, legal, it, analytics, product-engineering, executive, corporate)" },
                "offset": { "type": "integer", "description": "discover: page offset when browsing the whole catalog" },
                "name": { "type": "string", "description": "The employee's name" },
                "description": { "type": "string", "description": "The employee's description (create/update)" },
                "automations": {
                    "type": "array",
                    "description": "create/update: the employee's recurring or triggered duties. Each item becomes an employee workflow (Workflows tab + Schedule page, runs as the employee). Trigger is inferred from fields: schedule→cron, interval→heartbeat, sources→event. Steps must be CONCRETE and actionable — name the tools/files/destinations, what to check, what to produce, and how to report — the workflow runs unattended with only these words as its instructions. Simple duty: use steps (they run in order as one execution). Multi-stage duty (e.g. research at 2am feeding a publish at 10am, or triage→execute): use activities — each activity is its own scoped execution with its own intent + steps. On update this REPLACES ALL existing automations — use add_automations to append.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Workflow name, e.g. weekday-page-check" },
                            "schedule": { "type": "string", "description": "Cron (\"0 9 * * 1-5\") or human phrase (\"weekdays at 9am\", \"daily at 7am\") — auto-normalized" },
                            "interval": { "type": "string", "description": "Heartbeat interval (\"15m\", \"1h\")" },
                            "window": { "type": "string", "description": "Active window for interval, e.g. \"08:00-18:00\"" },
                            "sources": { "type": "array", "items": { "type": "string" }, "description": "Event sources that trigger it, e.g. \"email.received\"" },
                            "steps": { "type": "array", "items": { "type": "string" }, "description": "Concrete ordered steps for a single-stage duty. Executed in order in ONE run with shared context. Each step: what to do, with what tool/data, producing what output." },
                            "activities": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" }, "intent": { "type": "string", "description": "One line: what this stage accomplishes" }, "steps": { "type": "array", "items": { "type": "string" } }, "skills": { "type": "array", "items": { "type": "string" }, "description": "Skill names this stage may use" } }, "required": ["id", "intent", "steps"] }, "description": "Multi-stage form (overrides steps): sequential stages, each a separate scoped execution — later stages see earlier stages' outputs" },
                            "emit": { "type": "string", "description": "Event source name to emit on completion" },
                            "description": { "type": "string" }
                        },
                        "required": ["name"]
                    }
                },
                "add_automations": { "type": "array", "items": { "type": "object" }, "description": "update: ADD workflows to an existing employee without touching the others (same item shape as automations)" },
                "remove_automations": { "type": "array", "items": { "type": "string" }, "description": "update: remove workflows by name (also removes their schedules)" },
                "agent_md": { "type": "string", "description": "AGENT.md persona markdown (create/update; optional — name+description alone auto-generate it)" }
            },
            "required": ["action"]
        })
    }

    fn search_hint(&self) -> &str {
        "hire list create update employees"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        matches!(Self::action(input), "list" | "info" | "stats" | "discover")
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        match Self::action(input) {
            "list" => "list_employees",
            "info" => "get_employee",
            "discover" => "find_employees",
            "install" => "hire_employee",
            "create" => "create_employee",
            "update" => "update_employee",
            "delete" => "delete_employee",
            "activate" | "deactivate" => "set_employee_active",
            "setup" => "setup_employee",
            "repair" => "repair_employee",
            "reload" => "reload_employee",
            "stats" => "employee_stats",
            _ => "agent",
        }
        .to_string()
    }

    /// Pre-interface: it dispatches on `action` (see
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
            match input.get("resource").and_then(|v| v.as_str()) {
                None | Some("") | Some("registry") => {}
                Some(other) => {
                    return ToolResult::error(format!(
                        "The agent tool manages employees only; '{other}' is not one of its resources."
                    ));
                }
            }
            match Self::action(&input) {
                "" => ToolResult::error("action is required: discover, list, info, install, create, update, delete, activate, deactivate, setup, repair, reload or stats."),
                // discover is the one action that can park on a hire card,
                // so it needs the context; the rest do not.
                "discover" => self.persona.handle_discover(&input, ctx).await,
                _ => self.persona.handle_action(&input).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // The tool the model calls is `agent` (this one). PersonaTool registers as
    // `agents` and is not in the model's list, so hiring guidance written there
    // is never read — that is exactly what happened on 2026-09-14: discover
    // existed, compiled, passed its tests, and the model kept saying the
    // marketplace was empty. The description the model sees must carry it.
    #[test]
    fn the_agent_tool_the_model_sees_advertises_hiring() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("hire.db").to_string_lossy()).unwrap());
        let loader = Arc::new(napp::AgentLoader::new(dir.path().join("a"), dir.path().join("b")));
        let registry = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let tool = AgentTool::new(crate::agent_tool::PersonaTool::new(store, registry, loader));
        let d = tool.description();
        assert!(d.contains("action: \"discover\""), "discover is not advertised on the agent tool");
        assert!(d.contains("STAFFING"), "no staffing guidance on the agent tool");
        assert!(d.contains("[NeboAI]"), "does not say NeboAI's own employees come first");
        assert!(d.contains("the employee is the hire"), "does not say the employee is the hire and a tool is what they use");
        assert!(d.contains("Never search roles one at a time"), "does not tell the model to search every role in one call");
        // The description used to promise both catalogs in one sentence and
        // deny it in the next ("discover shows both" / "with no query the
        // page lists EMPLOYEES ONLY ... a separate catalog behind a different
        // door"). Browse searches both now, and the door is said once, from
        // the one constant (2026-09-19).
        assert!(
            d.contains(crate::plugin_tool::TOOL_INSTALL_DOOR),
            "the tool catalog's door is not the one constant"
        );
        for contradiction in ["EMPLOYEES ONLY", "separate catalog", "different door"] {
            assert!(!d.contains(contradiction), "the description still says `{contradiction}`");
        }
        for moved in ["resource: \"memory\"", "resource: \"task\"", "resource: \"ask\""] {
            assert!(!d.contains(moved), "the agent tool still describes {moved}");
        }
    }
}
