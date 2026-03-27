use std::sync::Arc;

use ai::{EmbeddingProvider, Provider};
use db::models::{ChatMessage, Memory};
use db::Store;
use tracing::{debug, warn};

use crate::chunking;
use crate::sanitize;

/// A single extracted fact from conversation.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Fact {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// Whether the user explicitly stated this fact (vs inferred).
    #[serde(default)]
    pub explicit: Option<bool>,
}

fn default_confidence() -> f64 {
    0.75
}

/// All facts extracted from a conversation.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ExtractedFacts {
    #[serde(default)]
    pub preferences: Vec<Fact>,
    #[serde(default)]
    pub entities: Vec<Fact>,
    #[serde(default)]
    pub decisions: Vec<Fact>,
    #[serde(default)]
    pub styles: Vec<Fact>,
    #[serde(default)]
    pub artifacts: Vec<Fact>,
    #[serde(default)]
    pub task_context: Vec<Fact>,
}

/// A storage-ready memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub layer: String,
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub tags: Vec<String>,
    pub is_style: bool,
    pub confidence: f64,
}

/// A memory scored by decay and confidence for ranking.
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub memory: Memory,
    pub score: f64,
}

/// Minimum confidence threshold for tacit memories in prompt.
const MIN_CONFIDENCE_THRESHOLD: f64 = 0.65;

/// Max characters per message when building extraction prompt.
const MAX_CONTENT_PER_MESSAGE: usize = 500;
/// Max total characters for extraction prompt.
const MAX_CONVERSATION_CHARS: usize = 15000;

/// Resolve confidence from raw value and explicit flag.
/// Explicit facts get 0.9, inferred facts get 0.6, raw value used as fallback.
fn resolve_confidence(raw: f64, explicit: Option<bool>) -> f64 {
    match explicit {
        Some(true) => 0.9,
        Some(false) => 0.6,
        None => raw.clamp(0.0, 1.0),
    }
}

/// Compute a decay score based on access count and recency.
/// Score = access_count * 0.7^(days_since_access / 30)
pub fn decay_score(access_count: i64, accessed_at: Option<i64>) -> f64 {
    let count = (access_count.max(1)) as f64;
    let days = match accessed_at {
        Some(ts) if ts > 0 => {
            let now = chrono::Utc::now().timestamp();
            ((now - ts) as f64 / 86400.0).max(0.0)
        }
        _ => 0.0,
    };
    count * 0.7_f64.powf(days / 30.0)
}

/// Score a memory by combining confidence from metadata with decay.
pub fn score_memory(mem: &Memory) -> f64 {
    let confidence = extract_confidence_from_metadata(mem).unwrap_or(0.75);
    // Parse TEXT timestamp from SQLite (e.g. "2026-03-06 12:34:56") into epoch seconds
    let accessed_ts = mem.accessed_at.as_deref().and_then(|s| {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| dt.and_utc().timestamp())
    });
    let decay = decay_score(
        mem.access_count.unwrap_or(0),
        accessed_ts,
    );
    confidence * decay
}

/// Extract confidence value from memory metadata JSON.
fn extract_confidence_from_metadata(mem: &Memory) -> Option<f64> {
    let metadata = mem.metadata.as_ref()?;
    let obj: serde_json::Value = serde_json::from_str(metadata).ok()?;
    obj.get("confidence")?.as_f64()
}

/// Extract facts from conversation messages.
pub async fn extract_facts(
    provider: &dyn Provider,
    messages: &[ChatMessage],
) -> Option<ExtractedFacts> {
    let conversation = build_conversation_text(messages);
    if conversation.is_empty() {
        return None;
    }

    let prompt = format!(
        "Extract key facts from this conversation. Return a JSON object with these categories:\n\
         - preferences: user preferences and behaviors\n\
         - entities: people, places, things mentioned\n\
         - decisions: decisions made during the conversation\n\
         - styles: communication/personality observations\n\
         - artifacts: content produced for the user\n\
         - task_context: task parameters (dates, budgets, quantities)\n\n\
         Each fact should have: key (string), value (string), category (string), tags (string array), confidence (0-1), explicit (boolean — true if user directly stated it, false if inferred).\n\n\
         Conversation:\n{}\n\n\
         Return ONLY valid JSON, no markdown fences.",
        conversation
    );

    let req = ai::ChatRequest {
        messages: vec![ai::Message {
            role: "user".to_string(),
            content: prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.0,
        system: "You are a precise fact extractor. Return only valid JSON.".to_string(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
    };

    let mut rx = match provider.stream(&req).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("memory extraction provider error: {}", e);
            return None;
        }
    };

    let mut response = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == ai::StreamEventType::Text {
            response.push_str(&event.text);
        }
    }

    // Parse JSON response
    let json_str = extract_json_object(&response)?;
    match serde_json::from_str::<ExtractedFacts>(&json_str) {
        Ok(facts) => Some(facts),
        Err(e) => {
            warn!("failed to parse extracted facts: {}", e);
            None
        }
    }
}

/// Store extracted facts in the database.
/// Style facts go through reinforcement, others are upserted directly.
pub fn store_facts(store: &Store, facts: &ExtractedFacts, user_id: &str) {
    let entries = format_for_storage(facts);
    for entry in entries {
        // Skip entries that look like prompt injection
        if sanitize::detect_prompt_injection(&entry.key)
            || sanitize::detect_prompt_injection(&entry.value)
        {
            debug!(
                "skipping memory entry due to injection detection: {}",
                entry.key
            );
            continue;
        }

        // Route style observations through reinforcement
        if entry.is_style {
            store_style_observation(store, &entry, user_id);
            continue;
        }

        let tags_json = if entry.tags.is_empty() {
            None
        } else {
            serde_json::to_string(&entry.tags).ok()
        };

        let metadata = serde_json::json!({
            "confidence": entry.confidence,
        })
        .to_string();

        if let Err(e) = store.upsert_memory(
            &entry.namespace,
            &entry.key,
            &entry.value,
            tags_json.as_deref(),
            Some(&metadata),
            user_id,
        ) {
            debug!(
                "failed to store memory entry {}/{}: {}",
                entry.namespace, entry.key, e
            );
        }
    }
}

/// Store a style observation with reinforcement.
/// If the observation already exists, increment reinforced_count and boost confidence
/// instead of overwriting the value.
fn store_style_observation(store: &Store, entry: &MemoryEntry, user_id: &str) {
    let existing = store
        .get_memory_by_key_and_user(&entry.namespace, &entry.key, user_id)
        .ok()
        .flatten();

    match existing {
        Some(mem) => {
            // Reinforce existing observation
            let mut meta: serde_json::Value = mem
                .metadata
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            let old_confidence = meta
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.75);
            let reinforced_count = meta
                .get("reinforced_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(1)
                + 1;

            // Boost confidence: new = old + (1 - old) * 0.2
            let new_confidence = (old_confidence + (1.0 - old_confidence) * 0.2).min(1.0);

            meta["confidence"] = serde_json::json!(new_confidence);
            meta["reinforced_count"] = serde_json::json!(reinforced_count);
            meta["last_reinforced"] = serde_json::json!(chrono::Utc::now().timestamp());

            let metadata_str = meta.to_string();
            if let Err(e) = store.update_memory_metadata(mem.id, &metadata_str) {
                debug!(
                    "failed to reinforce style {}/{}: {}",
                    entry.namespace, entry.key, e
                );
            }
        }
        None => {
            // New style observation
            let tags_json = if entry.tags.is_empty() {
                None
            } else {
                serde_json::to_string(&entry.tags).ok()
            };

            let metadata = serde_json::json!({
                "confidence": entry.confidence,
                "reinforced_count": 1,
                "first_observed": chrono::Utc::now().timestamp(),
                "last_reinforced": chrono::Utc::now().timestamp(),
            })
            .to_string();

            if let Err(e) = store.upsert_memory(
                &entry.namespace,
                &entry.key,
                &entry.value,
                tags_json.as_deref(),
                Some(&metadata),
                user_id,
            ) {
                debug!(
                    "failed to store style {}/{}: {}",
                    entry.namespace, entry.key, e
                );
            }
        }
    }
}

/// Convert extracted facts to storage entries.
fn format_for_storage(facts: &ExtractedFacts) -> Vec<MemoryEntry> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut entries = Vec::new();

    for f in &facts.preferences {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "tacit".to_string(),
            namespace: "tacit/preferences".to_string(),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: false,
            confidence,
        });
    }

    for f in &facts.entities {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "entity".to_string(),
            namespace: "entity/default".to_string(),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: false,
            confidence,
        });
    }

    for f in &facts.decisions {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "daily".to_string(),
            namespace: format!("daily/{}", today),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: false,
            confidence,
        });
    }

    for f in &facts.styles {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "tacit".to_string(),
            namespace: "tacit/personality".to_string(),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: true,
            confidence,
        });
    }

    for f in &facts.artifacts {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "tacit".to_string(),
            namespace: "tacit/artifacts".to_string(),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: false,
            confidence,
        });
    }

    for f in &facts.task_context {
        let confidence = resolve_confidence(f.confidence, f.explicit);
        entries.push(MemoryEntry {
            layer: "daily".to_string(),
            namespace: format!("daily/{}", today),
            key: sanitize::sanitize_memory_key(&normalize_key(&f.key)),
            value: sanitize::sanitize_memory_value(&f.value),
            tags: f.tags.clone(),
            is_style: false,
            confidence,
        });
    }

    entries
}

/// Normalize a memory key: lowercase, underscores/spaces to hyphens.
fn normalize_key(key: &str) -> String {
    key.to_lowercase()
        .replace('_', "-")
        .replace(' ', "-")
        .replace("--", "-")
        .replace("//", "/")
        .trim_matches('-')
        .trim_matches('/')
        .to_string()
}

/// Build conversation text for extraction, truncating per message and total.
fn build_conversation_text(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    let mut total_chars = 0usize;

    for msg in messages.iter().rev() {
        // Skip tool results
        if msg.role == "tool" {
            continue;
        }
        if msg.content.is_empty() {
            continue;
        }

        let content = if msg.content.len() > MAX_CONTENT_PER_MESSAGE {
            let mut end = MAX_CONTENT_PER_MESSAGE;
            while !msg.content.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &msg.content[..end])
        } else {
            msg.content.clone()
        };

        let line = format!("{}: {}", msg.role, content);
        total_chars += line.len();

        if total_chars > MAX_CONVERSATION_CHARS {
            break;
        }

        parts.push(line);
    }

    // Reverse to get chronological order (we built from the end)
    parts.reverse();
    parts.join("\n")
}

/// Find the first balanced JSON object in a response string.
fn extract_json_object(text: &str) -> Option<String> {
    // Strip markdown fences
    let text = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Find first { and match to closing }
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;

    for i in start..bytes.len() {
        let c = bytes[i];

        if escape {
            escape = false;
            continue;
        }

        if c == b'\\' && in_string {
            escape = true;
            continue;
        }

        if c == b'"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

/// Score and rank memories, returning top N by confidence * decay.
fn rank_memories(memories: Vec<Memory>, limit: usize) -> Vec<ScoredMemory> {
    let mut scored: Vec<ScoredMemory> = memories
        .into_iter()
        .map(|m| {
            let score = score_memory(&m);
            ScoredMemory { memory: m, score }
        })
        .collect();
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// Load memory context for a user from the database.
/// Uses two-pass overfetch with decay scoring to select the most relevant memories.
pub fn load_memory_context(store: &Store, user_id: &str) -> String {
    let mut all_scored: Vec<ScoredMemory> = Vec::new();
    let max_total: usize = 40;

    // Pass 1: tacit/personality — overfetch 30, cap 10
    if let Ok(memories) = store.get_tacit_memories_with_min_confidence(
        user_id,
        "tacit/personality",
        MIN_CONFIDENCE_THRESHOLD,
        30,
    ) {
        let ranked = rank_memories(memories, 10);
        all_scored.extend(ranked);
    }

    // Pass 2: other tacit/* — overfetch 120, fill remaining up to max_total
    let remaining = max_total.saturating_sub(all_scored.len());
    if remaining > 0 {
        if let Ok(memories) = store.get_tacit_memories_with_min_confidence(
            user_id,
            "tacit/",
            MIN_CONFIDENCE_THRESHOLD,
            120,
        ) {
            // Exclude personality memories already included
            let personality_ids: std::collections::HashSet<i64> =
                all_scored.iter().map(|s| s.memory.id).collect();
            let filtered: Vec<Memory> = memories
                .into_iter()
                .filter(|m| !personality_ids.contains(&m.id))
                .collect();
            let ranked = rank_memories(filtered, remaining);
            all_scored.extend(ranked);
        }
    }

    // Pass 3: daily memories (today)
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let daily_ns = format!("daily/{}", today);
    if let Ok(memories) = store.list_memories_by_user_and_namespace(user_id, &daily_ns, 20, 0) {
        let ranked = rank_memories(memories, 15);
        all_scored.extend(ranked);
    }

    // Pass 4: entity memories
    if let Ok(memories) = store.list_memories_by_user_and_namespace(user_id, "entity/", 30, 0) {
        let ranked = rank_memories(memories, 15);
        all_scored.extend(ranked);
    }

    // Final sort by score
    all_scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Format into sections
    let mut context_parts = Vec::new();

    let tacit: Vec<&ScoredMemory> = all_scored
        .iter()
        .filter(|s| s.memory.namespace.starts_with("tacit/"))
        .collect();
    if !tacit.is_empty() {
        let mut section = String::from("## Long-term memories\n");
        for s in &tacit {
            section.push_str(&format!("- {}: {}\n", s.memory.key, s.memory.value));
        }
        context_parts.push(section);
    }

    let daily: Vec<&ScoredMemory> = all_scored
        .iter()
        .filter(|s| s.memory.namespace.starts_with("daily/"))
        .collect();
    if !daily.is_empty() {
        let mut section = String::from("## Today's context\n");
        for s in &daily {
            section.push_str(&format!("- {}: {}\n", s.memory.key, s.memory.value));
        }
        context_parts.push(section);
    }

    let entities: Vec<&ScoredMemory> = all_scored
        .iter()
        .filter(|s| s.memory.namespace.starts_with("entity/"))
        .collect();
    if !entities.is_empty() {
        let mut section = String::from("## People & entities\n");
        for s in &entities {
            section.push_str(&format!("- {}: {}\n", s.memory.key, s.memory.value));
        }
        context_parts.push(section);
    }

    context_parts.join("\n")
}

/// Load scored tacit memories for use in prompt assembly.
pub fn load_scored_memories(store: &Store, user_id: &str, limit: usize) -> Vec<ScoredMemory> {
    let mut all_scored: Vec<ScoredMemory> = Vec::new();

    // Tacit memories with minimum confidence
    if let Ok(memories) = store.get_tacit_memories_with_min_confidence(
        user_id,
        "tacit/",
        MIN_CONFIDENCE_THRESHOLD,
        (limit * 3) as i64,
    ) {
        all_scored = rank_memories(memories, limit);
    }

    all_scored
}

/// Spawn a background task to chunk and embed recently stored memories.
/// This is fire-and-forget: failures are logged but don't affect the caller.
pub fn embed_memories_async(
    store: Arc<Store>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    entries: Vec<MemoryEntry>,
    user_id: String,
) {
    tokio::spawn(async move {
        for entry in &entries {
            // Look up the memory we just stored
            let mem = match store.get_memory_by_key_and_user(&entry.namespace, &entry.key, &user_id)
            {
                Ok(Some(m)) => m,
                _ => continue,
            };

            let text = format!("{}: {}", mem.key, mem.value);
            let chunks = chunking::chunk_text_default(&text);
            let model = embedding_provider.id().to_string();
            let dims = embedding_provider.dimensions() as i64;

            // Collect chunk texts for batch embedding
            let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
            if chunk_texts.is_empty() {
                continue;
            }

            let embeddings = match embedding_provider.embed(&chunk_texts).await {
                Ok(e) => e,
                Err(e) => {
                    debug!(
                        key = %entry.key,
                        error = %e,
                        "failed to embed memory"
                    );
                    continue;
                }
            };

            for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                let chunk_id = match store.insert_memory_chunk(
                    Some(mem.id),
                    i as i64,
                    &chunk.text,
                    "memory",
                    "",
                    chunk.start_char as i64,
                    chunk.end_char as i64,
                    &model,
                    &user_id,
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        debug!(error = %e, "failed to insert memory chunk");
                        continue;
                    }
                };

                let blob = ai::f32_to_bytes(embedding);
                if let Err(e) = store.insert_memory_embedding(chunk_id, &model, dims, &blob) {
                    debug!(error = %e, "failed to insert memory embedding");
                }
            }
        }
        debug!(count = entries.len(), "background embedding complete");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_key() {
        assert_eq!(normalize_key("User Name"), "user-name");
        assert_eq!(normalize_key("some_key"), "some-key");
        assert_eq!(normalize_key("a//b--c"), "a/b-c");
    }

    #[test]
    fn test_extract_json_object() {
        let text = r#"Here is the result: {"key": "value", "nested": {"a": 1}}"#;
        let json = extract_json_object(text).unwrap();
        assert_eq!(json, r#"{"key": "value", "nested": {"a": 1}}"#);
    }

    #[test]
    fn test_extract_json_with_fences() {
        let text = "```json\n{\"key\": \"value\"}\n```";
        let json = extract_json_object(text).unwrap();
        assert_eq!(json, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_resolve_confidence_explicit_true() {
        assert!((resolve_confidence(0.5, Some(true)) - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolve_confidence_explicit_false() {
        assert!((resolve_confidence(0.5, Some(false)) - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolve_confidence_none_uses_raw() {
        assert!((resolve_confidence(0.8, None) - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolve_confidence_clamps() {
        assert!((resolve_confidence(1.5, None) - 1.0).abs() < f64::EPSILON);
        assert!((resolve_confidence(-0.5, None) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decay_score_recent() {
        // Recently accessed with count 5 => ~5.0
        let now = chrono::Utc::now().timestamp();
        let score = decay_score(5, Some(now));
        assert!((score - 5.0).abs() < 0.1);
    }

    #[test]
    fn test_decay_score_old() {
        // Accessed 90 days ago with count 5 => 5 * 0.7^3 ≈ 1.715
        let ninety_days_ago = chrono::Utc::now().timestamp() - (90 * 86400);
        let score = decay_score(5, Some(ninety_days_ago));
        assert!(score < 2.0);
        assert!(score > 1.5);
    }

    #[test]
    fn test_decay_score_never_accessed() {
        let score = decay_score(1, None);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_rank_memories_sorts_by_score() {
        let mem1 = Memory {
            id: 1,
            namespace: "tacit/preferences".to_string(),
            key: "a".to_string(),
            value: "v".to_string(),
            tags: None,
            metadata: Some(r#"{"confidence": 0.9}"#.to_string()),
            created_at: None,
            updated_at: None,
            accessed_at: Some(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            access_count: Some(10),
            user_id: "u1".to_string(),
        };
        let mem2 = Memory {
            id: 2,
            namespace: "tacit/preferences".to_string(),
            key: "b".to_string(),
            value: "v".to_string(),
            tags: None,
            metadata: Some(r#"{"confidence": 0.5}"#.to_string()),
            created_at: None,
            updated_at: None,
            accessed_at: Some((chrono::Utc::now() - chrono::Duration::days(90)).format("%Y-%m-%d %H:%M:%S").to_string()),
            access_count: Some(1),
            user_id: "u1".to_string(),
        };

        let ranked = rank_memories(vec![mem2, mem1], 10);
        assert_eq!(ranked[0].memory.key, "a"); // higher score first
    }

    #[test]
    fn test_format_for_storage_applies_confidence() {
        let facts = ExtractedFacts {
            preferences: vec![Fact {
                key: "color".to_string(),
                value: "blue".to_string(),
                category: "preference".to_string(),
                tags: vec![],
                confidence: 0.5,
                explicit: Some(true),
            }],
            ..Default::default()
        };

        let entries = format_for_storage(&facts);
        assert_eq!(entries.len(), 1);
        assert!((entries[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_format_for_storage_sanitizes_keys() {
        let facts = ExtractedFacts {
            preferences: vec![Fact {
                key: "a\x00b".to_string(),
                value: "test\x01value".to_string(),
                category: "preference".to_string(),
                tags: vec![],
                confidence: 0.8,
                explicit: None,
            }],
            ..Default::default()
        };

        let entries = format_for_storage(&facts);
        assert!(!entries[0].key.contains('\x00'));
        assert!(!entries[0].value.contains('\x01'));
    }
}
