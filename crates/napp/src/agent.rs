//! Agent parser — AGENT.md (persona) and agent.json (workflow bindings + triggers).
//!
//! AGENT.md is pure prose — the agent's job description. No frontmatter required.
//!
//! agent.json carries the operational structure: inline workflow definitions
//! (activities, budgets, triggers) and what dependencies the agent requires.
//!
//! agent.json format:
//! ```json
//! {
//!   "workflows": {
//!     "morning-briefing": {
//!       "trigger": { "type": "schedule", "cron": "0 7 * * *" },
//!       "description": "Daily morning briefing",
//!       "activities": [{
//!         "id": "gather",
//!         "intent": "Gather news and calendar events"
//!       }],
//!       "budget": { "total_per_run": 5000 }
//!     }
//!   },
//!   "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
//!   "pricing": { "model": "monthly_fixed", "cost": 47.0 }
//! }
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::NappError;

// ---------------------------------------------------------------------------
// agent.json — workflow bindings, triggers, dependencies, pricing
// ---------------------------------------------------------------------------

/// Hard dependencies the agent requires (plugins, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRequires {
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// A sidecar tool definition declared in agent.json.
///
/// Each entry becomes a native tool registered for this agent. The LLM sees
/// `list_projects(...)` directly — calls are routed to the sidecar HTTP
/// endpoint specified by `method` + `path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolDef {
    /// Action name the LLM will use (e.g. "list_projects").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// HTTP method for the sidecar endpoint (GET, POST, PUT, DELETE).
    pub method: String,
    /// Sidecar-relative path, optionally with `{param}` placeholders.
    pub path: String,
    /// JSON Schema for input parameters (path params, query, body).
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
}

/// Named scope restricting which sidecar tools, skills, and plugins are
/// active when an embed chat is mounted with this scope name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolScope {
    /// Sidecar tool names active in this scope.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Skill refs to load (subset of top-level skills array).
    #[serde(default)]
    pub skills: Vec<String>,
    /// Additional plugin slugs to pre-activate for this scope.
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// Memory scoping configuration for an agent.
///
/// Controls how memories are isolated and inherited across the 3-tier hierarchy:
/// - Layer 1 (User):   `user_id = "user123"` — main Nebo companion
/// - Layer 2 (Agent):  `user_id = "user123:agent:brief"` — agent-wide
/// - Layer 3 (Context): `user_id = "user123:agent:brief:ctx:doc-123"` — per-context
///
/// Owner-scope inheritance is always on: every agent READS the owner's tacit
/// memories (facts about the owner belong to the owner, not the agent that
/// heard them) while WRITING only to its own scope. The former opt-in
/// `inherit_user` flag is gone; unknown fields in older agent.json parse fine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// When true, memories are isolated per contextId (from SDK embed sessions).
    /// Context comes from the session key's 4th segment: `agent:{id}:{channel}:{ctx}`.
    #[serde(default)]
    pub context_isolated: bool,
    /// Optional declared memory topics. When set, they replace the generic
    /// `project/` category in this agent's extraction prompt — each slug is a
    /// namespace prefix inside the agent's memory scope, and the description
    /// is injected verbatim as the category definition. Invariant layers
    /// (tacit/*, entity/) are never affected. Empty = default `project`.
    /// See docs/design/MEMORY_QUALITY.md.
    #[serde(default)]
    pub topics: Vec<MemoryTopic>,
}

/// One declared memory topic — `{ "slug": "lead", "description": "A prospective
/// buyer or seller — stage, budget, timeline, next action" }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTopic {
    pub slug: String,
    pub description: String,
}

/// Topic slugs that collide with built-in memory layers.
const RESERVED_TOPIC_SLUGS: &[&str] = &[
    "tacit", "entity", "daily", "project", "memory", "style", "artifact",
];

/// Max declared topics — bounds extraction-prompt growth (~150 tokens for 8).
const MAX_MEMORY_TOPICS: usize = 8;
/// Max topic description length in chars.
const MAX_TOPIC_DESCRIPTION_LEN: usize = 120;

fn is_kebab_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Agent configuration parsed from agent.json.
///
/// Contains the "schedule of intent" — which workflows run, when they fire,
/// and what dependencies the agent requires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub workflows: HashMap<String, WorkflowBinding>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub requires: AgentRequires,
    #[serde(default)]
    pub pricing: Option<AgentPricing>,
    #[serde(default)]
    pub defaults: Option<AgentDefaults>,
    /// Input field definitions — rendered as a dynamic form during setup.
    /// User-supplied values are stored and injected into workflow execution.
    #[serde(default)]
    pub inputs: Vec<AgentInputField>,
    /// Sidecar tool definitions. Each entry becomes a native tool routed to
    /// the sidecar HTTP endpoint. Follows the same filesystem-based pattern
    /// as skills and plugins — no HTTP discovery needed.
    #[serde(default)]
    pub tools: Vec<AgentToolDef>,
    /// Named tool scopes for SDK-driven filtering. Each scope maps to a subset
    /// of sidecar tools, skills, and plugins that are active when the embed
    /// chat is mounted with `scope: "<name>"`.
    #[serde(default)]
    pub scopes: HashMap<String, ToolScope>,
    /// Memory scoping configuration (inheritance + context isolation).
    #[serde(default)]
    pub memory: MemoryConfig,
}

/// A single input field the agent needs from the user.
///
/// Defines the schema for a dynamic form field rendered during agent setup.
/// Supported types: text, textarea, number, select, checkbox, radio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInputField {
    /// Unique key used to reference this value in workflows.
    /// Falls back to `name` if not provided (NeboAI uses `name`).
    #[serde(default)]
    pub key: String,
    /// Display label shown to the user.
    /// Falls back to empty (populated from `name` in post-processing).
    #[serde(default)]
    pub label: String,
    /// NeboAI uses `name` instead of `key` — accepted as alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional help text shown below the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Field type: text, textarea, number, select, checkbox, radio.
    #[serde(rename = "type", default = "default_input_type")]
    pub field_type: String,
    /// Whether the field must be filled before saving.
    #[serde(default)]
    pub required: bool,
    /// Default value (string, number, or bool depending on type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Placeholder text for text/textarea/number fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Options for select/radio fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AgentInputOption>,
}

/// An option in a select or radio field.
/// Accepts both `{ "value": "x", "label": "X" }` and plain `"x"` strings.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInputOption {
    pub value: String,
    pub label: String,
}

impl<'de> serde::Deserialize<'de> for AgentInputOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct OptionVisitor;

        impl<'de> de::Visitor<'de> for OptionVisitor {
            type Value = AgentInputOption;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or { value, label } object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<AgentInputOption, E> {
                let label = v.replace('_', " ").replace('-', " ");
                Ok(AgentInputOption {
                    value: v.to_string(),
                    label,
                })
            }

            fn visit_map<M: de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<AgentInputOption, M::Error> {
                #[derive(Deserialize)]
                struct Helper {
                    value: String,
                    #[serde(default)]
                    label: Option<String>,
                }
                let h = Helper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(AgentInputOption {
                    label: h.label.unwrap_or_else(|| h.value.clone()),
                    value: h.value,
                })
            }
        }

        deserializer.deserialize_any(OptionVisitor)
    }
}

fn default_input_type() -> String {
    "text".to_string()
}

/// An edge connecting two nodes in the workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConnection {
    /// Source node ID, `"__trigger__"`, or `"__emit__"`.
    pub from: String,
    /// Target node ID or `"__emit__"`.
    pub to: String,
    /// Branch label for condition/loop nodes: `"True"`, `"False"`, `"Each item"`, `"Done"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// An inline workflow bound to an agent with its trigger.
///
/// Activities, budget, and inputs are defined directly in agent.json.
/// No external workflow references — the agent owns the full procedure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBinding {
    /// When this workflow runs.
    pub trigger: AgentTrigger,
    /// Human-readable description of this binding.
    #[serde(default)]
    pub description: String,
    /// Default inputs passed to the workflow on trigger.
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,
    /// Inline activities (the procedure). Empty = chat-only binding.
    #[serde(default)]
    pub activities: Vec<AgentActivity>,
    /// Edges connecting activities (for the visual workflow builder).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<WorkflowConnection>,
    /// Budget constraints for the entire workflow run.
    #[serde(default)]
    pub budget: AgentBudget,
    /// Event name to emit on completion (e.g. "briefing.ready").
    /// Namespaced by agent slug at runtime: "agent-name.briefing.ready".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit: Option<String>,
}

impl WorkflowBinding {
    /// Serialize this binding into a workflow definition JSON string
    /// compatible with `workflow::parser::parse_workflow`.
    pub fn to_workflow_json(&self, name: &str) -> String {
        let inputs: HashMap<String, serde_json::Value> = self
            .inputs
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::json!({
                        "type": "string",
                        "default": v,
                    }),
                )
            })
            .collect();

        serde_json::json!({
            "version": "1.0",
            "id": name,
            "name": name,
            "inputs": inputs,
            "activities": self.activities,
            "connections": self.connections,
            "budget": self.budget,
            "dependencies": { "skills": [], "workflows": [] },
        })
        .to_string()
    }

    /// Returns true if this binding has inline activities to execute.
    pub fn has_activities(&self) -> bool {
        !self.activities.is_empty()
    }
}

/// A single activity in an agent's inline workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActivity {
    pub id: String,
    /// Activity type: custom, research, email, notify, code, condition, loop, wait, agent, connector, http, transform.
    #[serde(rename = "type", default)]
    pub activity_type: String,
    /// Natural-language task. Optional — typed nodes (http, wait, condition)
    /// may be fully described by `params`.
    #[serde(default)]
    pub intent: String,
    /// Display label from the builder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Declared branches for condition/loop activities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<AgentBranch>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcps: Vec<String>,
    #[serde(default)]
    pub cmds: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub token_budget: AgentTokenBudget,
    #[serde(default)]
    pub on_error: AgentOnError,
}

/// A declared branch on a branching activity (condition/loop).
/// Serializes as `{ label, nextId }` to match the builder and
/// `workflow::parser::Branch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBranch {
    pub label: String,
    #[serde(default, rename = "nextId", skip_serializing_if = "Option::is_none")]
    pub next_id: Option<String>,
}

/// Token budget for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTokenBudget {
    #[serde(default = "default_agent_token_max")]
    pub max: u32,
}

fn default_agent_token_max() -> u32 {
    4096
}

impl Default for AgentTokenBudget {
    fn default() -> Self {
        Self {
            max: default_agent_token_max(),
        }
    }
}

/// Error handling policy for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOnError {
    #[serde(default = "default_agent_retry")]
    pub retry: u32,
    #[serde(default = "default_agent_fallback")]
    pub fallback: AgentFallback,
}

fn default_agent_retry() -> u32 {
    1
}
fn default_agent_fallback() -> AgentFallback {
    AgentFallback::NotifyOwner
}

impl Default for AgentOnError {
    fn default() -> Self {
        Self {
            retry: default_agent_retry(),
            fallback: default_agent_fallback(),
        }
    }
}

/// Fallback strategy when an activity fails after all retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFallback {
    #[serde(alias = "NotifyOwner")]
    NotifyOwner,
    #[serde(alias = "Skip")]
    Skip,
    #[serde(alias = "Abort")]
    Abort,
}

/// Budget constraints for the entire workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentBudget {
    #[serde(default)]
    pub total_per_run: u32,
    #[serde(default)]
    pub cost_estimate: String,
}

/// Trigger types for agent-level workflow scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentTrigger {
    /// Cron-based, predictable schedule.
    #[serde(rename = "schedule")]
    Schedule {
        /// Cron expression (e.g. "0 8 * * *").
        #[serde(default)]
        cron: String,
        /// Human-readable schedule description (e.g. "8:00 AM daily").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schedule: Option<String>,
    },
    /// Recurring interval within a time window.
    #[serde(rename = "heartbeat")]
    Heartbeat {
        interval: String,
        #[serde(default)]
        window: Option<String>,
    },
    /// Event-driven, fires when something in the world changes.
    #[serde(rename = "event")]
    Event {
        #[serde(default)]
        sources: Vec<String>,
    },
    /// Watch trigger — spawns a plugin process that emits NDJSON to stdout.
    /// Each line triggers a workflow run with the parsed JSON as `_watch_payload`.
    /// When `event` is set, NDJSON output also auto-emits into the EventBus
    /// as `{plugin}.{event_name}`, and the command is resolved from the plugin
    /// manifest if not explicitly provided.
    #[serde(rename = "watch")]
    Watch {
        /// Plugin slug (e.g., "gws").
        plugin: String,
        /// CLI args passed to the plugin binary (e.g., "gmail +watch --project my-proj").
        /// Supports `{{key}}` template substitution from agent input values.
        /// Optional when `event` is set — resolved from plugin manifest.
        #[serde(default)]
        command: String,
        /// Plugin event name to watch (e.g., "email.new"). Enables auto-emission
        /// into EventBus as `{plugin}.{event_name}`. If set, the command can be
        /// omitted and will be resolved from the plugin manifest's event definition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event: Option<String>,
        /// Seconds to wait before restarting on crash (default: 5).
        #[serde(default = "default_restart_delay")]
        restart_delay_secs: u64,
    },
    /// Built-in folder watcher — uses OS filesystem notifications (notify crate)
    /// to detect new/changed files in a directory. No plugin binary needed.
    /// Each change batch triggers a workflow run with the affected file paths
    /// in `_watch_payload.files`.
    #[serde(rename = "folder")]
    Folder {
        /// Absolute path to watch (supports `{{key}}` template substitution).
        path: String,
        /// File extensions to match (e.g. ["pdf", "docx"]). Empty = all files.
        #[serde(default)]
        extensions: Vec<String>,
        /// Watch subdirectories recursively (default: true).
        #[serde(default = "default_true")]
        recursive: bool,
        /// Debounce window in seconds — aggregate rapid events into a single
        /// workflow trigger (default: 2).
        #[serde(default = "default_debounce_secs")]
        debounce_secs: u64,
    },
    /// Explicit user trigger.
    #[serde(rename = "manual")]
    Manual,
}

fn default_restart_delay() -> u64 {
    5
}

fn default_true() -> bool {
    true
}

fn default_debounce_secs() -> u64 {
    2
}

/// Pricing configuration for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPricing {
    pub model: String,
    #[serde(default)]
    pub cost: f64,
}

/// Default settings for the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    /// Timezone preference.
    #[serde(default)]
    pub timezone: String,
    /// JSON paths within agent.json that the user can override.
    #[serde(default)]
    pub configurable: Vec<String>,
}

/// Parse an agent.json file into an `AgentConfig`.
pub fn parse_agent_config(json_str: &str) -> Result<AgentConfig, NappError> {
    // Try strict parsing first; on failure, parse leniently by treating workflows
    // as raw JSON and skipping individual entries that don't match the schema.
    let mut config: AgentConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(_) => {
            // Parse as generic JSON, extract workflows separately
            let mut raw: serde_json::Value = serde_json::from_str(json_str)
                .map_err(|e| NappError::Manifest(format!("agent.json: {}", e)))?;
            // Remove workflows so the rest can parse cleanly
            let workflows_raw = raw.as_object_mut().and_then(|o| o.remove("workflows"));
            let mut cfg: AgentConfig = serde_json::from_value(raw)
                .map_err(|e| NappError::Manifest(format!("agent.json: {}", e)))?;
            // Try to parse each workflow individually, skipping failures
            if let Some(serde_json::Value::Object(map)) = workflows_raw {
                for (key, val) in map {
                    match serde_json::from_value::<crate::agent::WorkflowBinding>(val) {
                        Ok(wb) => { cfg.workflows.insert(key, wb); }
                        Err(e) => {
                            tracing::warn!(workflow = %key, error = %e, "skipping workflow with invalid schema");
                        }
                    }
                }
            }
            cfg
        }
    };

    // Normalize input fields: NeboAI uses `name` instead of `key`/`label`
    for field in &mut config.inputs {
        if field.key.is_empty() {
            if let Some(ref name) = field.name {
                field.key = name.clone();
            }
        }
        if field.label.is_empty() {
            let source = field.name.as_deref().unwrap_or(&field.key);
            field.label = source.replace('_', " ").replace('-', " ");
            // Capitalize first letter of each word
            field.label = field
                .label
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().to_string() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
    }

    // Normalize options: NeboAI may send plain strings instead of {value, label} objects
    // Options normalization: NeboAI may send plain strings instead of {value, label} objects.
    // AgentInputOption serde already handles this — plain strings fail serde and are handled by the caller.

    validate_agent_config(&config)?;
    Ok(config)
}

/// Check if a string is a valid skill reference: either a qualified name or install code.
fn is_qualified_skill_ref(s: &str) -> bool {
    // Accept install codes (SKIL-XXXX-XXXX)
    if s.starts_with("SKIL-") {
        return true;
    }

    // Accept qualified names (@org/skills/name or @org/skills/name@version)
    if !s.starts_with('@') {
        return false;
    }
    let without_at = &s[1..];
    let name_part = if let Some(idx) = without_at.find('@') {
        &without_at[..idx]
    } else {
        without_at
    };
    let segments: Vec<&str> = name_part.split('/').collect();
    if segments.len() != 3 {
        return false;
    }
    segments[1] == "skills" && !segments[0].is_empty() && !segments[2].is_empty()
}

/// Validate agent.json bindings.
fn validate_agent_config(config: &AgentConfig) -> Result<(), NappError> {
    for (name, binding) in &config.workflows {
        // Validate event triggers have at least one source
        if let AgentTrigger::Event { sources } = &binding.trigger {
            if sources.is_empty() {
                return Err(NappError::Manifest(format!(
                    "workflow '{}' event trigger must have at least one source",
                    name
                )));
            }
        }
        // Validate activity IDs are unique within a binding
        let mut seen = std::collections::HashSet::new();
        for activity in &binding.activities {
            if !activity.id.is_empty() && !seen.insert(&activity.id) {
                return Err(NappError::Manifest(format!(
                    "workflow '{}' has duplicate activity id: {}",
                    name, activity.id
                )));
            }
        }
    }
    for ref_str in &config.skills {
        if !is_qualified_skill_ref(ref_str) {
            tracing::warn!(skill_ref = %ref_str, "skill ref is not a qualified name — cascade install may skip it");
        }
    }
    if config.memory.topics.len() > MAX_MEMORY_TOPICS {
        return Err(NappError::Manifest(format!(
            "memory.topics may declare at most {} topics (got {})",
            MAX_MEMORY_TOPICS,
            config.memory.topics.len()
        )));
    }
    for topic in &config.memory.topics {
        if !is_kebab_slug(&topic.slug) {
            return Err(NappError::Manifest(format!(
                "memory topic slug '{}' must be kebab-case: lowercase letters, numbers, and hyphens",
                topic.slug
            )));
        }
        if RESERVED_TOPIC_SLUGS.contains(&topic.slug.as_str()) {
            return Err(NappError::Manifest(format!(
                "memory topic slug '{}' is reserved",
                topic.slug
            )));
        }
        if topic.description.trim().is_empty() || topic.description.len() > MAX_TOPIC_DESCRIPTION_LEN
        {
            return Err(NappError::Manifest(format!(
                "memory topic '{}' description must be non-empty and at most {} chars",
                topic.slug, MAX_TOPIC_DESCRIPTION_LEN
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AGENT.md — backward compat for frontmatter-based agents
// ---------------------------------------------------------------------------

/// Parsed AGENT.md frontmatter (backward compatibility).
///
/// New agents use pure prose in AGENT.md with no frontmatter. This struct
/// supports the legacy format where identity was in frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Markdown body after the frontmatter (not from YAML).
    #[serde(skip)]
    pub body: String,
}

/// Parse an AGENT.md file.
///
/// If the file has YAML frontmatter, extracts identity fields.
/// If the file is pure prose, returns empty identity with the full content as body.
pub fn parse_agent(content: &str) -> Result<AgentDef, NappError> {
    let trimmed = content.trim_start();

    // Pure prose — no frontmatter
    if !trimmed.starts_with("---") {
        return Ok(AgentDef {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            body: content.trim().to_string(),
        });
    }

    // Legacy frontmatter format
    let (yaml_str, body) = split_frontmatter(content)?;
    let mut def: AgentDef = serde_yaml::from_str(&yaml_str)
        .map_err(|e| NappError::Manifest(format!("agent YAML: {}", e)))?;
    def.body = body;
    Ok(def)
}

/// Split `---` delimited frontmatter from the markdown body.
/// Returns `(yaml_str, body)`. If no frontmatter found, yaml_str is empty.
pub fn split_frontmatter(content: &str) -> Result<(String, String), NappError> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return Ok((String::new(), trimmed.to_string()));
    }

    let after_first = trimmed[3..].trim_start();
    match after_first.find("\n---") {
        Some(pos) => {
            let yaml = after_first[..pos].trim().to_string();
            let body = after_first[pos + 4..].trim().to_string();
            Ok((yaml, body))
        }
        None => {
            // No closing delimiter — split at first blank line
            match after_first.find("\n\n") {
                Some(pos) => {
                    let yaml = after_first[..pos].trim().to_string();
                    let body = after_first[pos..].trim().to_string();
                    Ok((yaml, body))
                }
                None => Ok((String::new(), trimmed.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualified_skill_ref_validation() {
        assert!(is_qualified_skill_ref("@nebo/skills/briefing-writer"));
        assert!(is_qualified_skill_ref(
            "@nebo/skills/briefing-writer@^1.0.0"
        ));
        assert!(is_qualified_skill_ref("SKIL-ABCD-1234"));
        assert!(!is_qualified_skill_ref("bad-ref"));
        assert!(!is_qualified_skill_ref("@acme/tools/crm")); // wrong type
        assert!(!is_qualified_skill_ref("@/skills/name")); // empty org
        assert!(!is_qualified_skill_ref("@org/skills/")); // empty name
    }

    #[test]
    fn test_parse_agent_config_inline() {
        let json = r#"{
            "workflows": {
                "morning-briefing": {
                    "trigger": { "type": "schedule", "cron": "0 7 * * *" },
                    "description": "Daily morning briefing",
                    "activities": [{
                        "id": "gather",
                        "intent": "Gather news and calendar events",
                        "model": "sonnet",
                        "steps": ["Fetch top headlines", "Check today's calendar"]
                    }],
                    "budget": { "total_per_run": 5000 }
                },
                "day-monitor": {
                    "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
                },
                "interrupt": {
                    "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] }
                }
            },
            "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
            "pricing": { "model": "monthly_fixed", "cost": 47.0 },
            "defaults": {
                "timezone": "user_local",
                "configurable": ["workflows.morning-briefing.trigger.cron"]
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.workflows.len(), 3);

        let briefing = &config.workflows["morning-briefing"];
        assert!(matches!(briefing.trigger, AgentTrigger::Schedule { .. }));
        assert_eq!(briefing.description, "Daily morning briefing");
        assert_eq!(briefing.activities.len(), 1);
        assert_eq!(briefing.activities[0].id, "gather");
        assert_eq!(briefing.budget.total_per_run, 5000);

        let monitor = &config.workflows["day-monitor"];
        assert!(matches!(monitor.trigger, AgentTrigger::Heartbeat { .. }));
        assert!(monitor.activities.is_empty()); // chat-only binding

        let interrupt = &config.workflows["interrupt"];
        if let AgentTrigger::Event { sources } = &interrupt.trigger {
            assert_eq!(sources.len(), 2);
        } else {
            panic!("expected event trigger");
        }

        assert_eq!(config.skills, vec!["@nebo/skills/briefing-writer@^1.0.0"]);

        let pricing = config.pricing.unwrap();
        assert_eq!(pricing.model, "monthly_fixed");
        assert!((pricing.cost - 47.0).abs() < f64::EPSILON);

        let defaults = config.defaults.unwrap();
        assert_eq!(defaults.timezone, "user_local");
        assert_eq!(defaults.configurable.len(), 1);
    }

    #[test]
    fn test_manual_trigger() {
        let json = r#"{
            "workflows": {
                "ad-hoc": {
                    "trigger": { "type": "manual" }
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        assert!(matches!(
            config.workflows["ad-hoc"].trigger,
            AgentTrigger::Manual
        ));
    }

    #[test]
    fn test_watch_trigger() {
        let json = r#"{
            "workflows": {
                "inbox-watcher": {
                    "trigger": {
                        "type": "watch",
                        "plugin": "gws",
                        "command": "gmail +watch --project {{gcp_project}} --poll-interval 30"
                    }
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        let binding = &config.workflows["inbox-watcher"];
        match &binding.trigger {
            AgentTrigger::Watch {
                plugin,
                command,
                event,
                restart_delay_secs,
            } => {
                assert_eq!(plugin, "gws");
                assert!(command.contains("gmail +watch"));
                assert!(event.is_none());
                assert_eq!(*restart_delay_secs, 5); // default
            }
            other => panic!("expected Watch trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_watch_trigger_with_event() {
        let json = r#"{
            "workflows": {
                "inbox-watcher": {
                    "trigger": {
                        "type": "watch",
                        "plugin": "gws",
                        "event": "email.new"
                    }
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        let binding = &config.workflows["inbox-watcher"];
        match &binding.trigger {
            AgentTrigger::Watch {
                plugin,
                command,
                event,
                ..
            } => {
                assert_eq!(plugin, "gws");
                assert!(command.is_empty()); // resolved at runtime from manifest
                assert_eq!(event.as_deref(), Some("email.new"));
            }
            other => panic!("expected Watch trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_bad_skill_ref_warns_but_succeeds() {
        // Non-qualified skill refs warn but don't reject the config
        let json = r#"{"skills": ["BAD-prefix"]}"#;
        assert!(parse_agent_config(json).is_ok());
    }

    #[test]
    fn test_empty_event_sources() {
        let json = r#"{"workflows": {"x": {"trigger": {"type": "event", "sources": []}}}}"#;
        assert!(parse_agent_config(json).is_err());
    }

    #[test]
    fn test_duplicate_activity_ids() {
        let json = r#"{"workflows": {"x": {
            "trigger": {"type": "manual"},
            "activities": [
                {"id": "step1", "intent": "a"},
                {"id": "step1", "intent": "b"}
            ]
        }}}"#;
        assert!(parse_agent_config(json).is_err());
    }

    #[test]
    fn test_requires_plugins() {
        let json = r#"{"requires": {"plugins": ["PLUG-PJ3Z-ECFV"]}}"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.requires.plugins, vec!["PLUG-PJ3Z-ECFV"]);
    }

    #[test]
    fn test_requires_plugins_with_full_typeconfig() {
        // Simulates the full typeConfig returned by the NeboAI API for chief-of-staff.
        // This is the exact shape that goes through serde_json::to_string(type_config)
        // then into extract_agent_deps_from_frontmatter.
        let json = r#"{
            "inputs": [{"key": "name", "type": "text", "label": "Your name"}],
            "skills": [],
            "pricing": {"cost": 0.0, "model": "monthly_fixed"},
            "defaults": {"timezone": "user_local"},
            "requires": {"plugins": ["PLUG-PJ3Z-ECFV"]},
            "workflows": {
                "morning-briefing": {
                    "trigger": {"type": "schedule", "cron": "0 7 * * 1-5"},
                    "description": "Morning briefing",
                    "activities": [{"id": "triage", "intent": "Get unread email summary", "skills": ["gws-gmail-triage"], "model": "nebo-1", "steps": ["Run: gws gmail +triage"]}],
                    "budget": {"total_per_run": 8000}
                },
                "email-watcher": {
                    "trigger": {"type": "watch", "event": "email.new", "plugin": "gws", "restart_delay_secs": 5},
                    "description": "Relay new email events"
                },
                "check-inbox": {
                    "trigger": {"type": "manual"},
                    "description": "On-demand inbox check",
                    "activities": [{"id": "triage-unread", "intent": "Fetch unread", "skills": ["gws-gmail-triage"], "model": "nebo-1", "steps": ["Run triage"]}],
                    "budget": {"total_per_run": 12000}
                },
                "day-monitor": {
                    "trigger": {"type": "heartbeat", "interval": "30m", "window": "07:00-22:00"},
                    "description": "Watch for urgent emails",
                    "activities": [{"id": "quick-scan", "intent": "Fast check", "skills": ["gws-gmail-triage", "gws-calendar-agenda"], "model": "nebo-1", "steps": ["Quick scan"]}],
                    "budget": {"total_per_run": 3000}
                },
                "auto-reply": {
                    "trigger": {"type": "event", "sources": ["gws.email.new"]},
                    "description": "Auto-reply to common email types",
                    "activities": [{"id": "analyze-email", "intent": "Analyze", "skills": ["gws-gmail-read"], "model": "nebo-1", "steps": ["Read email"]}],
                    "budget": {"total_per_run": 5000}
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.requires.plugins, vec!["PLUG-PJ3Z-ECFV"]);
        assert_eq!(config.workflows.len(), 5);
    }

    #[test]
    fn test_requires_defaults_empty() {
        let json = "{}";
        let config = parse_agent_config(json).unwrap();
        assert!(config.requires.plugins.is_empty());
    }

    #[test]
    fn test_empty_agent_config() {
        let json = "{}";
        let config = parse_agent_config(json).unwrap();
        assert!(config.workflows.is_empty());
        assert!(config.skills.is_empty());
        assert!(config.pricing.is_none());
        assert!(config.scopes.is_empty());
        // Memory config defaults
        assert!(!config.memory.context_isolated);
        assert!(config.memory.topics.is_empty());
    }

    #[test]
    fn test_memory_topics_parse_and_validate() {
        let json = r#"{"memory": {"topics": [
            {"slug": "lead", "description": "A prospective buyer or seller — stage, budget, timeline, next action"},
            {"slug": "listing", "description": "A property being marketed — address, price, status, showings"}
        ]}}"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.memory.topics.len(), 2);
        assert_eq!(config.memory.topics[0].slug, "lead");
    }

    #[test]
    fn test_memory_topics_reject_invalid() {
        // Reserved slug
        let json = r#"{"memory": {"topics": [{"slug": "project", "description": "x"}]}}"#;
        assert!(parse_agent_config(json).is_err());
        // Non-kebab slug
        let json = r#"{"memory": {"topics": [{"slug": "My Leads", "description": "x"}]}}"#;
        assert!(parse_agent_config(json).is_err());
        // Empty description
        let json = r#"{"memory": {"topics": [{"slug": "lead", "description": "  "}]}}"#;
        assert!(parse_agent_config(json).is_err());
        // Over the topic cap
        let topics: Vec<String> = (0..9)
            .map(|i| format!(r#"{{"slug": "topic-{i}", "description": "d"}}"#))
            .collect();
        let json = format!(r#"{{"memory": {{"topics": [{}]}}}}"#, topics.join(","));
        assert!(parse_agent_config(&json).is_err());
        // Description over 120 chars
        let long = "d".repeat(121);
        let json = format!(r#"{{"memory": {{"topics": [{{"slug": "lead", "description": "{long}"}}]}}}}"#);
        assert!(parse_agent_config(&json).is_err());
    }

    #[test]
    fn test_memory_config_parsing() {
        // `inherit_user` was removed (owner-scope inheritance is always on);
        // older agent.json carrying it must still parse.
        let json = r#"{"memory": {"inherit_user": true, "context_isolated": true}}"#;
        let config = parse_agent_config(json).unwrap();
        assert!(config.memory.context_isolated);
    }

    #[test]
    fn test_memory_config_defaults() {
        let json = r#"{"memory": {}}"#;
        let config = parse_agent_config(json).unwrap();
        assert!(!config.memory.context_isolated);
    }


    #[test]
    fn test_tools_parsing() {
        let json = r#"{
            "tools": [
                {
                    "name": "list_projects",
                    "description": "List all projects",
                    "method": "GET",
                    "path": "/projects"
                },
                {
                    "name": "get_document",
                    "description": "Get a document by ID",
                    "method": "GET",
                    "path": "/documents/{id}",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" }
                        },
                        "required": ["id"]
                    }
                }
            ]
        }"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.tools.len(), 2);
        assert_eq!(config.tools[0].name, "list_projects");
        assert_eq!(config.tools[0].method, "GET");
        assert!(config.tools[0].input_schema.is_none());
        assert_eq!(config.tools[1].name, "get_document");
        assert!(config.tools[1].input_schema.is_some());
    }

    #[test]
    fn test_tools_default_empty() {
        let json = "{}";
        let config = parse_agent_config(json).unwrap();
        assert!(config.tools.is_empty());
    }

    #[test]
    fn test_scopes_parsing() {
        let json = r#"{
            "scopes": {
                "editor": {
                    "tools": ["get_document", "update_document"],
                    "skills": ["skills/document-editing"],
                    "plugins": ["gws"]
                },
                "projects": {
                    "tools": ["list_projects"]
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        assert_eq!(config.scopes.len(), 2);
        let editor = &config.scopes["editor"];
        assert_eq!(editor.tools, vec!["get_document", "update_document"]);
        assert_eq!(editor.skills, vec!["skills/document-editing"]);
        assert_eq!(editor.plugins, vec!["gws"]);
        let projects = &config.scopes["projects"];
        assert_eq!(projects.tools, vec!["list_projects"]);
        assert!(projects.skills.is_empty());
        assert!(projects.plugins.is_empty());
    }

    #[test]
    fn test_parse_pure_prose_agent_md() {
        let content = "# Chief of Staff\n\nYou manage the executive's daily rhythm.";
        let def = parse_agent(content).unwrap();
        assert!(def.id.is_empty());
        assert!(def.body.contains("Chief of Staff"));
    }

    #[test]
    fn test_parse_legacy_frontmatter_agent_md() {
        let content = "---\nid: sales-sdr\nname: Sales SDR\n---\n# Sales SDR\n\nBody text.";
        let def = parse_agent(content).unwrap();
        assert_eq!(def.id, "sales-sdr");
        assert_eq!(def.name, "Sales SDR");
        assert!(def.body.contains("Body text."));
    }

    #[test]
    fn test_workflow_binding_with_inputs() {
        let json = r#"{
            "workflows": {
                "daily-report": {
                    "trigger": { "type": "schedule", "cron": "0 9 * * *" },
                    "inputs": { "format": "brief", "include_charts": true }
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        let binding = &config.workflows["daily-report"];
        assert_eq!(binding.inputs.len(), 2);
        assert_eq!(binding.inputs["format"], "brief");
    }

    #[test]
    fn test_inline_activities() {
        let json = r#"{
            "workflows": {
                "test-flow": {
                    "trigger": { "type": "manual" },
                    "activities": [{
                        "id": "step1",
                        "intent": "Do something",
                        "model": "sonnet",
                        "steps": ["Step one"]
                    }],
                    "budget": { "total_per_run": 3000 }
                }
            }
        }"#;
        let config = parse_agent_config(json).unwrap();
        let binding = &config.workflows["test-flow"];
        assert_eq!(binding.activities.len(), 1);
        assert_eq!(binding.activities[0].id, "step1");
        assert_eq!(binding.activities[0].intent, "Do something");
        assert_eq!(binding.budget.total_per_run, 3000);
    }
}
