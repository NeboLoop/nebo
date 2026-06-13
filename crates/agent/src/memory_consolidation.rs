//! Background memory consolidation — periodically reviews, deduplicates, and
//! prunes memories within each user_id scope.
//!
//! Gate chain (cheapest first):
//! 1. Feature enabled in settings
//! 2. Time gate: >= 24h since last consolidation for this scope
//! 3. Memory count threshold: >= 20 memories in the scope
//! 4. In-memory mutex per scope (single process, no file lock needed)
//!
//! Each distinct `user_id` is consolidated independently. Cross-scope merging
//! never happens — `"user:agent:brief:ctx:doc-A"` is never merged with `:ctx:doc-B`.
//!
//! Consolidation is the system's ONLY memory reaper (docs/design/MEMORY_QUALITY.md):
//! there is no TTL machinery. Topic memories (`project/`, agent-declared slugs)
//! are retired here when their work is done or dated.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use ai::{Provider, StreamEventType};
use db::Store;

/// Minimum memories in a scope before consolidation is worth running.
const MIN_MEMORIES_FOR_CONSOLIDATION: i64 = 20;
/// Hours between consolidation runs per scope.
const CONSOLIDATION_INTERVAL_HOURS: i64 = 24;
/// Interval between background sweeps (minutes).
const SWEEP_INTERVAL_MINUTES: u64 = 30;

/// Tracks last consolidation time per scope.
static LAST_CONSOLIDATION: LazyLock<Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-flight consolidation locks per scope.
static SCOPE_LOCKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Result of a single scope consolidation.
#[derive(Debug, Default)]
pub struct ConsolidationResult {
    pub kept: usize,
    pub updated: usize,
    pub deleted: usize,
}

/// Spawn the background consolidation sweep task.
/// Runs every 30 minutes, iterates over distinct user_id scopes, and
/// consolidates each that passes the gate chain.
pub fn spawn_sweep(store: Arc<Store>, providers: Arc<tokio::sync::RwLock<Vec<Arc<dyn Provider>>>>) {
    tokio::spawn(async move {
        // Retire the legacy daily/ layer (docs/design/MEMORY_QUALITY.md):
        // date-scoped memories are gone — ongoing work lives in topical
        // layers. Cheap SQL, idempotent; runs at startup so old installs
        // converge without any TTL machinery.
        match store.delete_memories_by_namespace_prefix("daily/") {
            Ok(0) => {}
            Ok(n) => info!(deleted = n, "retired legacy daily/ memory layer"),
            Err(e) => warn!(error = %e, "daily layer retirement sweep failed"),
        }

        // Let server boot before expensive background LLM work.
        // tokio::time::interval first tick fires immediately — this delay
        // prevents ~30s of LLM calls during the startup window.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(SWEEP_INTERVAL_MINUTES * 60));

        loop {
            interval.tick().await;

            // Gate 1: check if consolidation is enabled
            if !is_enabled(&store) {
                continue;
            }

            let scopes = match store.get_distinct_memory_user_ids() {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "memory consolidation: failed to get scopes");
                    continue;
                }
            };

            for (user_id, count) in &scopes {
                // Gate 3: minimum memory count
                if *count < MIN_MEMORIES_FOR_CONSOLIDATION {
                    continue;
                }

                // Gate 2: time since last consolidation
                if !should_consolidate(user_id).await {
                    continue;
                }

                // Gate 4: acquire scope lock (non-blocking)
                {
                    let mut locks = SCOPE_LOCKS.lock().await;
                    if locks.contains(user_id) {
                        continue; // already in progress
                    }
                    locks.insert(user_id.clone());
                }

                let provider = {
                    let prov_lock = providers.read().await;
                    crate::runner::prefer_non_gateway(&prov_lock)
                };

                if let Some(provider) = provider {
                    match consolidate_scope(provider.as_ref(), &store, user_id).await {
                        Ok(result) => {
                            info!(
                                scope = %user_id,
                                kept = result.kept,
                                updated = result.updated,
                                deleted = result.deleted,
                                "memory consolidation complete"
                            );
                        }
                        Err(e) => {
                            warn!(scope = %user_id, error = %e, "memory consolidation failed");
                        }
                    }
                }

                // Release scope lock
                {
                    let mut locks = SCOPE_LOCKS.lock().await;
                    locks.remove(user_id);
                }

                // Record consolidation time
                {
                    let mut last = LAST_CONSOLIDATION.lock().await;
                    last.insert(user_id.clone(), chrono::Utc::now());
                }
            }
        }
    });
}

/// Check whether enough time has passed since last consolidation for this scope.
async fn should_consolidate(user_id: &str) -> bool {
    let last = LAST_CONSOLIDATION.lock().await;
    match last.get(user_id) {
        Some(ts) => {
            let elapsed = chrono::Utc::now() - *ts;
            elapsed.num_hours() >= CONSOLIDATION_INTERVAL_HOURS
        }
        None => true, // never consolidated
    }
}

/// Check if auto-consolidation is enabled in settings.
fn is_enabled(store: &Store) -> bool {
    // Default to enabled; users can disable via plugin_settings
    store
        .get_plugin_setting("nebo", "memory_consolidation")
        .ok()
        .flatten()
        .map(|v| v != "disabled")
        .unwrap_or(true)
}

/// Run memory consolidation for a single user_id scope.
/// Loads all memories, asks LLM to merge duplicates, resolve contradictions,
/// and prune stale facts. Applies updates via upsert/delete.
pub async fn consolidate_scope(
    provider: &dyn Provider,
    store: &Store,
    user_id: &str,
) -> Result<ConsolidationResult, String> {
    // Load ALL memories for this scope (tacit, topical, entity) — consolidation
    // is the only reaper, so topic layers must be visible to it.
    let memories = store
        .list_memories_by_user_and_namespace(user_id, "", 200, 0)
        .map_err(|e| format!("load memories: {}", e))?;

    if memories.len() < MIN_MEMORIES_FOR_CONSOLIDATION as usize {
        return Ok(ConsolidationResult::default());
    }

    // Build the memory list for the LLM
    let mut memory_lines = Vec::new();
    for mem in &memories {
        memory_lines.push(format!(
            "{{\"id\": {}, \"ns\": \"{}\", \"key\": \"{}\", \"value\": \"{}\"}}",
            mem.id,
            mem.namespace,
            mem.key.replace('"', "\\\""),
            mem.value.replace('"', "\\\"")
        ));
    }

    let prompt = format!(
        "You are a memory curator. Review these {} facts stored for a user and:\n\
         1. Identify duplicates — keep the most complete version, mark others for deletion\n\
         2. Identify contradictions — newer facts (higher id) supersede older ones\n\
         3. Separate the durable from the dated — preferences, working style, and key\n\
            relationships (tacit/ and entity/ namespaces) are durable: keep and sharpen them.\n\
            Topic facts (project/ and other namespaces) describe ongoing work: if the work\n\
            is clearly done or its date has passed, delete them — folding any lasting\n\
            takeaway into an updated durable fact\n\
         4. Identify stale or irrelevant facts that should be removed\n\n\
         Facts:\n[{}]\n\n\
         Return ONLY valid JSON with this structure:\n\
         {{\n\
           \"keep\": [<ids to keep unchanged>],\n\
           \"update\": [{{\"id\": <id>, \"value\": \"<new merged value>\"}}],\n\
           \"delete\": [<ids to remove>]\n\
         }}\n\n\
         Rules:\n\
         - Every input id must appear in exactly one of: keep, update (by id), or delete\n\
         - Bias toward keeping — only delete when clearly redundant, contradicted, done, or dated\n\
         - When merging duplicates, apply the update to the OLDEST id (preserves the original\n\
           created date) and delete the newer copies\n\
         - Updated values should be concise declarative facts",
        memories.len(),
        memory_lines.join(",\n")
    );

    let req = ai::ChatRequest {
        tool_choice: Default::default(),
        messages: vec![ai::Message {
            role: "user".to_string(),
            content: prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.0,
        system: "You are a precise memory curator. Return only valid JSON.".to_string(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = provider
        .stream(&req)
        .await
        .map_err(|e| format!("provider error: {}", e))?;

    let mut response = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == StreamEventType::Text {
            response.push_str(&event.text);
        }
    }

    // Parse the consolidation response
    let json_str = crate::memory::extract_json_object_pub(&response)
        .ok_or_else(|| "no JSON object in consolidation response".to_string())?;

    let plan: ConsolidationPlan = serde_json::from_str(&json_str)
        .map_err(|e| format!("parse consolidation plan: {}", e))?;

    let mut result = ConsolidationResult::default();

    // Apply updates
    for update in &plan.update {
        if let Err(e) = store.update_memory(update.id, Some(&update.value), None, None) {
            debug!(id = update.id, error = %e, "consolidation: update failed");
        } else {
            result.updated += 1;
        }
    }

    // Apply deletions
    for id in &plan.delete {
        if let Err(e) = store.delete_memory(*id) {
            debug!(id, error = %e, "consolidation: delete failed");
        } else {
            result.deleted += 1;
        }
    }

    result.kept = plan.keep.len();
    Ok(result)
}

#[derive(Debug, serde::Deserialize)]
struct ConsolidationPlan {
    #[serde(default)]
    keep: Vec<i64>,
    #[serde(default)]
    update: Vec<ConsolidationUpdate>,
    #[serde(default)]
    delete: Vec<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct ConsolidationUpdate {
    id: i64,
    value: String,
}
