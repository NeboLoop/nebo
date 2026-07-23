//! Background memory consolidation — periodically reviews, deduplicates, and
//! prunes memories within each user_id scope — plus the write-time
//! contradiction micro-check (Phase 2, docs/plans/memory-rock-solid.md).
//!
//! Sweep gate chain (cheapest first):
//! 1. Feature enabled in settings
//! 2. Scope holds at least a PAIR of memories (curation is relational)
//! 3. Time gate: >= 24h since last consolidation for this scope
//! 4. Activity gate: >= N distinct write bursts since last consolidation
//!    (`memory_scope_activity`, recorded at the ONE upsert funnel) — activity,
//!    not size, marks a scope as live
//! 5. In-memory mutex per scope (single process, no file lock needed)
//!
//! Each distinct `user_id` is consolidated independently. Cross-scope merging
//! never happens — `"user:agent:brief:ctx:doc-A"` is never merged with `:ctx:doc-B`.
//!
//! Consolidation is the system's ONLY memory reaper (docs/design/MEMORY_QUALITY.md):
//! there is no TTL machinery. Topic memories (`project/`, agent-declared slugs)
//! are retired here when their work is done or dated.
//!
//! The micro-check complements the sweep: on every NEW memory write (via the
//! canonical embed wrapper), the freshly embedded fact is compared against its
//! OWN scope's vectors; a near-duplicate under a different key triggers ONE
//! cheap LLM call on just that pair — so a wrong fact in a 3-memory case scope
//! gets corrected the moment its replacement arrives, not at sweep time.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use ai::{EmbeddingProvider, Provider, StreamEventType};
use db::Store;
use db::models::Memory;

/// Curation operates on relationships BETWEEN facts (duplicates,
/// contradictions, merges) — a scope needs at least a pair before a curator
/// call can do anything. Deliberately not a size gate: correctness does not
/// scale with row count.
const MIN_MEMORIES_FOR_CONSOLIDATION: i64 = 2;
/// Distinct write bursts (`memory_scope_activity`, one burst ≈ one
/// session/run that touched the scope) since the last consolidation before
/// the curator runs. Activity means the scope is live and worth one curator
/// call; a dormant scope — however large — never burns one. 3 bursts keeps a
/// single stray write from triggering a full review.
const MIN_WRITE_BURSTS_FOR_CONSOLIDATION: i64 = 3;
/// Hours between consolidation runs per scope.
const CONSOLIDATION_INTERVAL_HOURS: i64 = 24;
/// Interval between background sweeps (minutes).
const SWEEP_INTERVAL_MINUTES: u64 = 30;
/// Cosine similarity above which a newly stored fact is put in front of the
/// micro-curator together with the nearest existing same-scope fact. 0.88 is
/// deliberately high: only near-restatements of the same subject clear it
/// ("settlement figure is $4.2M" vs "settlement figure is $3.1M"), while
/// merely related facts in the same case never do — a false trigger costs an
/// LLM call and risks a bad merge, a miss just waits for the sweep.
const CONTRADICTION_SIMILARITY: f64 = 0.88;

/// Tracks last consolidation time per scope.
static LAST_CONSOLIDATION: LazyLock<Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-flight consolidation locks per scope.
static SCOPE_LOCKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// In-flight micro-check locks per scope — at most one write-time
/// contradiction check runs per scope at a time; concurrent stores simply
/// skip (the sweep is the backstop).
static MICRO_LOCKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Chat-provider handle for the write-time micro-curator, set once by
/// [`spawn_sweep`] (the same wiring that powers the sweep). The memory write
/// paths deliberately don't carry chat providers — when this is unset (unit
/// tests, no server wiring) the micro-check silently skips.
static PROVIDERS: OnceLock<Arc<tokio::sync::RwLock<Vec<Arc<dyn Provider>>>>> = OnceLock::new();

/// Result of a single scope consolidation.
#[derive(Debug, Default)]
pub struct ConsolidationResult {
    pub kept: usize,
    pub updated: usize,
    pub deleted: usize,
}

/// The ONE provider/model resolution for every curator call (sweep and
/// micro-check): honor aux task-routing (`task_routing.aux` in models.yaml)
/// when configured, otherwise prefer a non-gateway provider with its default
/// model — the sweep's historical selection.
fn curation_provider(providers: &[Arc<dyn Provider>]) -> Option<(Arc<dyn Provider>, String)> {
    crate::runner::resolve_aux(&config::ModelsConfig::load(), providers)
        .or_else(|| crate::runner::prefer_non_gateway(providers).map(|p| (p, String::new())))
}

/// Spawn the background consolidation sweep task.
/// Runs every 30 minutes, iterates over distinct user_id scopes, and
/// consolidates each that passes the gate chain. `embedding_provider` keeps
/// the vector store truthful after curation: merged values are re-embedded
/// through the canonical pathway (None = FTS-only install, nothing to refresh).
pub fn spawn_sweep(
    store: Arc<Store>,
    providers: Arc<tokio::sync::RwLock<Vec<Arc<dyn Provider>>>>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
) {
    // Share the provider handle with the write-time micro-curator.
    let _ = PROVIDERS.set(providers.clone());

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
                // Gate 2: curation needs at least a pair of facts
                if *count < MIN_MEMORIES_FOR_CONSOLIDATION {
                    continue;
                }

                // Gate 3: time since last consolidation
                if !should_consolidate(user_id).await {
                    continue;
                }

                // Gate 4: activity — enough distinct write bursts since the
                // scope's last consolidation. Replaces the old ≥20-memories
                // size gate, which starved small-but-live scopes (a per-case
                // ctx scope holds a handful of rows) while dormant large
                // scopes kept burning curator calls after restarts.
                match store.get_memory_scope_write_events(user_id) {
                    Ok(events) if events >= MIN_WRITE_BURSTS_FOR_CONSOLIDATION => {}
                    Ok(_) => continue,
                    Err(e) => {
                        warn!(scope = %user_id, error = %e, "memory consolidation: activity check failed");
                        continue;
                    }
                }

                // Gate 5: acquire scope lock (non-blocking)
                {
                    let mut locks = SCOPE_LOCKS.lock().await;
                    if locks.contains(user_id) {
                        continue; // already in progress
                    }
                    locks.insert(user_id.clone());
                }

                let provider = {
                    let prov_lock = providers.read().await;
                    curation_provider(&prov_lock)
                };

                if let Some((provider, model)) = provider {
                    match consolidate_scope(
                        provider.as_ref(),
                        &model,
                        &store,
                        user_id,
                        embedding_provider.as_deref(),
                    )
                    .await
                    {
                        Ok(result) => {
                            info!(
                                scope = %user_id,
                                kept = result.kept,
                                updated = result.updated,
                                deleted = result.deleted,
                                "memory consolidation complete"
                            );
                            // Restart the activity window: the gate measures
                            // "since last consolidation".
                            if let Err(e) = store.reset_memory_scope_activity(user_id) {
                                warn!(scope = %user_id, error = %e, "failed to reset scope activity");
                            }
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
    model: &str,
    store: &Store,
    user_id: &str,
    embedding_provider: Option<&dyn EmbeddingProvider>,
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

    let response = run_curation_prompt(provider, model, prompt).await?;

    // Parse the consolidation response
    let json_str = crate::memory::extract_json_object_pub(&response)
        .ok_or_else(|| "no JSON object in consolidation response".to_string())?;

    let plan: ConsolidationPlan = serde_json::from_str(&json_str)
        .map_err(|e| format!("parse consolidation plan: {}", e))?;

    Ok(apply_consolidation_plan(store, user_id, &memories, &plan, embedding_provider).await)
}

/// Apply a curation plan to the scope. Only ids present in `memories` (the
/// scope's own loaded rows) are ever touched — a hallucinated id can never
/// reach another scope's memory. After deletions (chunk + embedding rows
/// cascade away) or merges (values re-embedded through the ONE canonical
/// pathway), the scope's cached ANN index is dropped so vector recall never
/// serves deleted or stale vectors within the same server lifetime.
async fn apply_consolidation_plan(
    store: &Store,
    user_id: &str,
    memories: &[Memory],
    plan: &ConsolidationPlan,
    embedding_provider: Option<&dyn EmbeddingProvider>,
) -> ConsolidationResult {
    let by_id: HashMap<i64, &Memory> = memories.iter().map(|m| (m.id, m)).collect();
    let mut result = ConsolidationResult::default();
    let mut updated_keys: Vec<(String, String)> = Vec::new();

    // Apply updates
    for update in &plan.update {
        let Some(mem) = by_id.get(&update.id) else {
            debug!(id = update.id, "consolidation: ignoring out-of-scope update id");
            continue;
        };
        if let Err(e) = store.update_memory(update.id, Some(&update.value), None, None) {
            debug!(id = update.id, error = %e, "consolidation: update failed");
        } else {
            result.updated += 1;
            updated_keys.push((mem.namespace.clone(), mem.key.clone()));
        }
    }

    // Apply deletions
    for id in &plan.delete {
        if !by_id.contains_key(id) {
            debug!(id, "consolidation: ignoring out-of-scope delete id");
            continue;
        }
        if let Err(e) = store.delete_memory(*id) {
            debug!(id, error = %e, "consolidation: delete failed");
        } else {
            result.deleted += 1;
        }
    }

    result.kept = plan.keep.len();

    if result.updated > 0 || result.deleted > 0 {
        // Merged values get fresh vectors (embed_memories clears the stale
        // chunks and invalidates the index itself on success)…
        if let Some(ep) = embedding_provider {
            if !updated_keys.is_empty() {
                crate::memory::embed_memories(store, ep, &updated_keys, user_id).await;
            }
        }
        // …and deletions must drop the cached index too — their DB rows
        // cascade away, but a cached index would keep serving them.
        crate::search_adapter::invalidate_index(user_id);
    }

    result
}

/// The ONE LLM plumbing for curator calls (sweep and micro-check): build the
/// curation request, stream, and collect the text response.
async fn run_curation_prompt(
    provider: &dyn Provider,
    model: &str,
    prompt: String,
) -> Result<String, String> {
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
        model: model.to_string(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
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
    Ok(response)
}

/// Write-time contradiction micro-check for freshly stored + embedded
/// memories (Phase 2, docs/plans/memory-rock-solid.md). Called from the ONE
/// new-write embed wrapper (`memory::embed_memories_async`) — best-effort in
/// every direction: no providers wired, no embeddings, a busy scope, or any
/// error simply skips the check (the sweep is the backstop). Never blocks or
/// fails a store.
pub async fn check_new_memories(
    store: &Arc<Store>,
    embedding_provider: &dyn EmbeddingProvider,
    keys: &[(String, String)],
    user_id: &str,
) {
    let Some(providers) = PROVIDERS.get() else {
        return; // no server wiring (tests, embedded use) — skip
    };
    let resolved = {
        let lock = providers.read().await;
        curation_provider(&lock)
    };
    let Some((provider, model)) = resolved else {
        return; // no chat provider — skippable by design
    };

    // Rate limit: at most one in-flight micro-check per scope.
    {
        let mut locks = MICRO_LOCKS.lock().await;
        if !locks.insert(user_id.to_string()) {
            return;
        }
    }

    micro_check_scope(
        store,
        provider.as_ref(),
        &model,
        embedding_provider,
        keys,
        user_id,
    )
    .await;

    MICRO_LOCKS.lock().await.remove(user_id);
}

/// Compare each newly written memory against its OWN scope's vector rows and
/// micro-curate the top near-duplicate pair, if any.
async fn micro_check_scope(
    store: &Arc<Store>,
    provider: &dyn Provider,
    model: &str,
    embedding_provider: &dyn EmbeddingProvider,
    keys: &[(String, String)],
    user_id: &str,
) {
    let embed_model = embedding_provider.id().to_string();
    for (namespace, key) in keys {
        let Ok(Some(new_mem)) = store.get_memory_by_key_and_user(namespace, key, user_id) else {
            continue;
        };
        // Reload per key: an applied verdict mutates the scope, and the
        // per-scope row count is small enough that a fresh exact-scope query
        // is cheaper than reasoning about staleness.
        let scope_embeddings = match store.list_memory_embeddings_by_user(user_id, &embed_model) {
            Ok(rows) if !rows.is_empty() => rows,
            _ => return,
        };
        let Some((candidate_id, similarity)) =
            find_contradiction_candidate(&scope_embeddings, new_mem.id, &new_mem.key)
        else {
            continue;
        };
        let Ok(Some(existing)) = store.get_memory(candidate_id) else {
            continue;
        };
        // Belt and braces: candidates come from an exact-scope query, but a
        // curator must never operate on a row outside its scope.
        if existing.user_id != user_id {
            continue;
        }
        debug!(
            scope = %user_id,
            new_key = %new_mem.key,
            existing_key = %existing.key,
            similarity,
            "write-time contradiction candidate"
        );
        if let Err(e) = micro_curate_pair(
            provider,
            model,
            store,
            embedding_provider,
            &existing,
            &new_mem,
            user_id,
        )
        .await
        {
            warn!(scope = %user_id, error = %e, "micro-curation failed");
        }
    }
}

/// Top contradiction candidate for a newly stored memory within ONE scope's
/// embeddings: the existing memory (under a DIFFERENT key — same key is just
/// the memory's own upsert) whose vector is most cosine-similar to the new
/// memory's, when that similarity clears [`CONTRADICTION_SIMILARITY`].
/// Callers pass rows from an exact-`user_id` query, so a ctx scope can never
/// be compared against a sibling scope.
fn find_contradiction_candidate(
    scope_embeddings: &[(i64, String, Vec<u8>)],
    new_id: i64,
    new_key: &str,
) -> Option<(i64, f64)> {
    let new_vecs: Vec<Vec<f32>> = scope_embeddings
        .iter()
        .filter(|(id, _, _)| *id == new_id)
        .map(|(_, _, blob)| ai::bytes_to_f32(blob))
        .collect();
    if new_vecs.is_empty() {
        return None;
    }

    let mut best: Option<(i64, f64)> = None;
    for (id, key, blob) in scope_embeddings {
        if *id == new_id || key == new_key {
            continue;
        }
        let existing_vec = ai::bytes_to_f32(blob);
        for new_vec in &new_vecs {
            let sim = crate::search::cosine_similarity(new_vec, &existing_vec);
            if sim >= CONTRADICTION_SIMILARITY && best.is_none_or(|(_, b)| sim > b) {
                best = Some((*id, sim));
            }
        }
    }
    best
}

/// ONE cheap LLM call on a single (existing, new) pair — merge / supersede /
/// keep_both — applied through the same store methods the sweep curator uses.
async fn micro_curate_pair(
    provider: &dyn Provider,
    model: &str,
    store: &Store,
    embedding_provider: &dyn EmbeddingProvider,
    existing: &Memory,
    newly: &Memory,
    user_id: &str,
) -> Result<(), String> {
    let prompt = format!(
        "Two memories in the same scope are semantically close. Decide how to reconcile them.\n\n\
         EXISTING (id {}, key \"{}\"): \"{}\"\n\
         NEW (id {}, key \"{}\", just stored): \"{}\"\n\n\
         Return ONLY valid JSON, exactly one of:\n\
         {{\"action\": \"merge\", \"value\": \"<one concise fact combining both>\"}} — they describe the same thing and can be one fact\n\
         {{\"action\": \"supersede\"}} — the NEW fact contradicts or replaces the EXISTING one (the existing will be deleted)\n\
         {{\"action\": \"keep_both\"}} — genuinely distinct facts\n\n\
         Bias toward keep_both unless they clearly cover the same subject.",
        existing.id,
        existing.key,
        existing.value.replace('"', "\\\""),
        newly.id,
        newly.key,
        newly.value.replace('"', "\\\"")
    );

    let response = run_curation_prompt(provider, model, prompt).await?;
    let json_str = crate::memory::extract_json_object_pub(&response)
        .ok_or_else(|| "no JSON object in micro-curation response".to_string())?;
    let verdict: MicroVerdict =
        serde_json::from_str(&json_str).map_err(|e| format!("parse micro verdict: {}", e))?;

    apply_micro_verdict(
        store,
        &verdict,
        existing,
        newly,
        Some(embedding_provider),
        user_id,
    )
    .await;
    Ok(())
}

/// Apply a micro-curation verdict via the existing store methods, then keep
/// the vector store truthful: merged survivors are re-embedded through the
/// canonical pathway and the scope's cached index is dropped (deleted rows
/// cascade their chunk/embedding rows away, but a cached index would keep
/// serving them).
async fn apply_micro_verdict(
    store: &Store,
    verdict: &MicroVerdict,
    existing: &Memory,
    newly: &Memory,
    embedding_provider: Option<&dyn EmbeddingProvider>,
    user_id: &str,
) {
    match verdict.action.as_str() {
        "merge" => {
            let Some(value) = verdict.value.as_deref().filter(|v| !v.is_empty()) else {
                debug!(scope = %user_id, "micro-curation: merge verdict without value — kept both");
                return;
            };
            // Same rule as the sweep curator: the merged value lands on the
            // OLDEST id (preserves the original created date); the newer copy
            // is deleted.
            let (target, dropped) = if existing.id <= newly.id {
                (existing, newly)
            } else {
                (newly, existing)
            };
            if let Err(e) = store.update_memory(target.id, Some(value), None, None) {
                debug!(id = target.id, error = %e, "micro-curation: merge update failed");
                return;
            }
            if let Err(e) = store.delete_memory(dropped.id) {
                debug!(id = dropped.id, error = %e, "micro-curation: merge delete failed");
            }
            if let Some(ep) = embedding_provider {
                crate::memory::embed_memories(
                    store,
                    ep,
                    &[(target.namespace.clone(), target.key.clone())],
                    user_id,
                )
                .await;
            }
            info!(scope = %user_id, kept = %target.key, dropped = %dropped.key, "micro-curation: merged pair");
        }
        "supersede" => {
            if let Err(e) = store.delete_memory(existing.id) {
                debug!(id = existing.id, error = %e, "micro-curation: supersede delete failed");
                return;
            }
            info!(scope = %user_id, superseded = %existing.key, by = %newly.key, "micro-curation: superseded");
        }
        _ => {
            debug!(scope = %user_id, "micro-curation: keeping both");
            return;
        }
    }
    crate::search_adapter::invalidate_index(user_id);
}

#[derive(Debug, serde::Deserialize)]
struct MicroVerdict {
    action: String,
    #[serde(default)]
    value: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> (Arc<Store>, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("nebo-consol-{}-{}.db", name, std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Arc::new(Store::new(&path.to_string_lossy()).unwrap());
        (store, path)
    }

    fn mem(store: &Store, ns: &str, key: &str, value: &str, user_id: &str) -> Memory {
        store.upsert_memory(ns, key, value, None, None, user_id).unwrap();
        store
            .get_memory_by_key_and_user(ns, key, user_id)
            .unwrap()
            .unwrap()
    }

    /// Insert one chunk + embedding for a memory (test embed model "m").
    fn embed(store: &Store, memory: &Memory, vec: &[f32]) {
        let chunk = store
            .insert_memory_chunk(
                Some(memory.id),
                0,
                &format!("{}: {}", memory.key, memory.value),
                "memory",
                "",
                0,
                0,
                "m",
                &memory.user_id,
            )
            .unwrap();
        store
            .insert_memory_embedding(chunk, "m", vec.len() as i64, &ai::f32_to_bytes(vec))
            .unwrap();
    }

    // ── Activity gate ──────────────────────────────────────────────────

    #[test]
    fn test_activity_gate_small_active_scope_eligible_big_dormant_not() {
        let (store, path) = test_store("gate");

        // Small ctx scope (5 memories) written by 3 separated bursts →
        // eligible: activity, not size, is the gate.
        let active = "gate-owner:agent:a1:ctx:case-A";
        for i in 0..5 {
            store
                .upsert_memory("project", &format!("fact-{i}"), "v", None, None, active)
                .unwrap();
        }
        // The rapid batch above is ONE burst; add two more (gap 0 = each counts).
        store.record_memory_write_activity(active, 0).unwrap();
        store.record_memory_write_activity(active, 0).unwrap();
        assert!(
            store.get_memory_scope_write_events(active).unwrap()
                >= MIN_WRITE_BURSTS_FOR_CONSOLIDATION,
            "small but active scope must pass the activity gate"
        );

        // Big scope (30 memories) with no activity since its last
        // consolidation → NOT eligible, however large.
        let dormant = "gate-owner:agent:a2";
        for i in 0..30 {
            store
                .upsert_memory("project", &format!("fact-{i}"), "v", None, None, dormant)
                .unwrap();
        }
        store.reset_memory_scope_activity(dormant).unwrap();
        assert!(
            store.get_memory_scope_write_events(dormant).unwrap()
                < MIN_WRITE_BURSTS_FOR_CONSOLIDATION,
            "dormant scope must not pass the activity gate regardless of size"
        );

        let _ = std::fs::remove_file(&path);
    }

    // ── Contradiction candidate ────────────────────────────────────────

    /// Unit vector with cosine `c` against [1, 0, 0, 0].
    fn vec_with_cos(c: f32) -> Vec<f32> {
        vec![c, (1.0 - c * c).sqrt(), 0.0, 0.0]
    }

    #[test]
    fn test_find_contradiction_candidate_threshold() {
        let base = ai::f32_to_bytes(&[1.0, 0.0, 0.0, 0.0]);
        let close = ai::f32_to_bytes(&vec_with_cos(0.95)); // above 0.88
        let related = ai::f32_to_bytes(&vec_with_cos(0.70)); // below 0.88
        let orthogonal = ai::f32_to_bytes(&[0.0, 1.0, 0.0, 0.0]);

        // Near-restatement under a different key triggers…
        let rows = vec![
            (1_i64, "settlement-figure".to_string(), base.clone()),
            (2_i64, "settlement-amount".to_string(), close.clone()),
            (3_i64, "opposing-counsel".to_string(), orthogonal.clone()),
        ];
        let hit = find_contradiction_candidate(&rows, 1, "settlement-figure");
        assert_eq!(hit.map(|(id, _)| id), Some(2));

        // …merely related and unrelated facts never do.
        let rows = vec![
            (1_i64, "settlement-figure".to_string(), base.clone()),
            (2_i64, "filing-deadline".to_string(), related),
            (3_i64, "opposing-counsel".to_string(), orthogonal),
        ];
        assert!(find_contradiction_candidate(&rows, 1, "settlement-figure").is_none());

        // Same key is the memory's own upsert — never a candidate.
        let rows = vec![
            (1_i64, "settlement-figure".to_string(), base.clone()),
            (2_i64, "settlement-figure".to_string(), close),
        ];
        assert!(find_contradiction_candidate(&rows, 1, "settlement-figure").is_none());

        // No embedding for the new memory → nothing to compare.
        let rows = vec![(2_i64, "other".to_string(), base)];
        assert!(find_contradiction_candidate(&rows, 1, "settlement-figure").is_none());
    }

    #[test]
    fn test_contradiction_check_never_crosses_sibling_scopes() {
        let (store, path) = test_store("isolation");
        let scope_a = "iso-owner:agent:a1:ctx:case-A";
        let scope_b = "iso-owner:agent:a1:ctx:case-B";

        // Identical vectors in sibling ctx scopes — the classic bleed-through
        // trap. The exact-scope query must only surface the scope's own rows.
        let vec = [1.0_f32, 0.0, 0.0, 0.0];
        let mem_a = mem(&store, "project", "settlement", "Case A settles at $4.2M", scope_a);
        embed(&store, &mem_a, &vec);
        let mem_b = mem(&store, "project", "figure", "Settlement figure is $3.1M", scope_b);
        embed(&store, &mem_b, &vec);

        let rows_b = store.list_memory_embeddings_by_user(scope_b, "m").unwrap();
        assert_eq!(rows_b.len(), 1, "exact-scope query must exclude sibling scopes");
        assert_eq!(rows_b[0].0, mem_b.id);

        // With only its own row visible, the new memory has no candidate —
        // Case A's near-identical fact is structurally unreachable.
        assert!(find_contradiction_candidate(&rows_b, mem_b.id, &mem_b.key).is_none());

        let _ = std::fs::remove_file(&path);
    }

    // ── Verdict application ────────────────────────────────────────────

    #[tokio::test]
    async fn test_apply_micro_verdict_merge_supersede_keep_both() {
        let (store, path) = test_store("verdict");
        let scope = "verdict-owner:agent:a1:ctx:case-A";

        // merge: merged value lands on the oldest id, newer copy deleted.
        let older = mem(&store, "project", "budget", "Budget is $5,000", scope);
        let newer = mem(&store, "project", "budget-update", "Budget raised to $8,000", scope);
        let verdict = MicroVerdict {
            action: "merge".to_string(),
            value: Some("Budget was $5,000, raised to $8,000".to_string()),
        };
        apply_micro_verdict(&store, &verdict, &older, &newer, None, scope).await;
        let survivor = store.get_memory(older.id).unwrap().unwrap();
        assert_eq!(survivor.value, "Budget was $5,000, raised to $8,000");
        assert!(store.get_memory(newer.id).unwrap().is_none(), "newer copy deleted");

        // supersede: the existing (contradicted) memory is deleted, new kept.
        let stale = mem(&store, "project", "deadline", "Filing deadline is March 3", scope);
        let fresh = mem(&store, "project", "deadline-new", "Filing deadline moved to April 7", scope);
        let verdict = MicroVerdict { action: "supersede".to_string(), value: None };
        apply_micro_verdict(&store, &verdict, &stale, &fresh, None, scope).await;
        assert!(store.get_memory(stale.id).unwrap().is_none(), "superseded memory deleted");
        assert!(store.get_memory(fresh.id).unwrap().is_some(), "new memory kept");

        // keep_both: nothing changes.
        let a = mem(&store, "project", "judge", "Judge is Hon. Rivera", scope);
        let b = mem(&store, "project", "courtroom", "Hearings are in courtroom 4B", scope);
        let verdict = MicroVerdict { action: "keep_both".to_string(), value: None };
        apply_micro_verdict(&store, &verdict, &a, &b, None, scope).await;
        assert!(store.get_memory(a.id).unwrap().is_some());
        assert!(store.get_memory(b.id).unwrap().is_some());

        // merge without a value is a no-op (never delete on a malformed verdict).
        let verdict = MicroVerdict { action: "merge".to_string(), value: None };
        apply_micro_verdict(&store, &verdict, &a, &b, None, scope).await;
        assert!(store.get_memory(a.id).unwrap().is_some());
        assert!(store.get_memory(b.id).unwrap().is_some());

        let _ = std::fs::remove_file(&path);
    }

    // ── Consolidation apply: scope safety + index freshness ────────────

    /// Same unit vector for every text: vector search finds exactly what the
    /// index contains (mirrors the search_adapter freshness test).
    struct ConstEmbedder;

    #[async_trait::async_trait]
    impl ai::EmbeddingProvider for ConstEmbedder {
        fn id(&self) -> &str {
            "const-consol-embed"
        }
        fn dimensions(&self) -> usize {
            8
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ai::ProviderError> {
            Ok(texts.iter().map(|_| vec![1.0; 8]).collect())
        }
    }

    #[tokio::test]
    async fn test_apply_plan_invalidates_index_after_delete_and_skips_foreign_ids() {
        use tools::bot_tool::HybridSearcher;

        let (store, path) = test_store("applyplan");
        // Unique scope: the index cache is process-global, shared across tests.
        let scope = "applyplan-owner:agent:a1";
        let foreign_scope = "applyplan-owner:agent:OTHER";
        let provider: Arc<dyn ai::EmbeddingProvider> = Arc::new(ConstEmbedder);

        // Two memories embedded through the canonical pathway, plus one in a
        // FOREIGN scope the plan must never be able to touch.
        let keep = mem(&store, "project", "fact-keep", "the kept fact", scope);
        let drop = mem(&store, "project", "fact-drop", "the deleted fact", scope);
        let foreign = mem(&store, "project", "foreign-fact", "another scope's fact", foreign_scope);
        crate::memory::embed_memories(
            &store,
            provider.as_ref(),
            &[
                ("project".to_string(), "fact-keep".to_string()),
                ("project".to_string(), "fact-drop".to_string()),
            ],
            scope,
        )
        .await;

        // Build + cache the scope's vector index.
        let adapter =
            crate::search_adapter::HybridSearchAdapter::new(store.clone(), Some(provider.clone()));
        let results = adapter.search("zzxx qqvv", scope, 10, Some(0.0)).await;
        assert!(
            results.iter().any(|r| r.memory_id == Some(drop.id)),
            "seed search must see both memories via the vector index"
        );

        // Plan deletes one memory — and hallucinates the foreign scope's id.
        let memories = store.list_memories_by_user_and_namespace(scope, "", 100, 0).unwrap();
        let plan = ConsolidationPlan {
            keep: vec![keep.id],
            update: vec![],
            delete: vec![drop.id, foreign.id],
        };
        let result = apply_consolidation_plan(&store, scope, &memories, &plan, None).await;
        assert_eq!(result.deleted, 1, "foreign id must be ignored");
        assert!(store.get_memory(foreign.id).unwrap().is_some(), "foreign scope untouched");

        // The cached index must have been dropped: a rebuilt index no longer
        // contains the deleted memory (its chunk/embedding rows cascaded away).
        let results = adapter.search("zzxx qqvv", scope, 10, Some(0.0)).await;
        assert!(
            !results.iter().any(|r| r.memory_id == Some(drop.id)),
            "deleted memory must vanish from vector search without a restart; got {results:?}"
        );
        assert!(
            results.iter().any(|r| r.memory_id == Some(keep.id)),
            "kept memory still searchable"
        );

        let _ = std::fs::remove_file(&path);
    }
}
