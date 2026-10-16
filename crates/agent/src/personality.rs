use ai::{Provider, StreamEventType};
use sha2::Digest;
use db::Store;
use tracing::{debug, info, warn};

/// Minimum number of style observations before synthesizing a directive.
const MIN_OBSERVATIONS: usize = 5;
/// Maximum observations to include in synthesis prompt.
const MAX_OBSERVATIONS: usize = 15;
/// Base lifespan multiplier per reinforced_count (days).
const LIFESPAN_DAYS_PER_REINFORCEMENT: i64 = 14;

/// Where the directive is stored, inside the observations' namespace.
const DIRECTIVE_NAMESPACE: &str = "tacit/personality";
const DIRECTIVE_KEY: &str = "directive";

/// Synthesize a personality directive from style observations.
/// Calls the LLM to produce a 3-5 sentence directive in 2nd person.
/// Stores the result as `tacit/personality/directive`, with the hash of the
/// observations it was written from (`input_hash`): while the observations
/// are unchanged, the stored directive stands and no call is made (audit
/// 2026-10-16: ~95 syntheses a day on unchanged input).
pub async fn synthesize_directive(
    store: &Store,
    provider: &dyn Provider,
    model: &str,
    user_id: &str,
) -> Option<String> {
    // Every observation under `tacit/personality` (the style ones included:
    // the namespace is a prefix), never the directive itself, which would
    // feed each synthesis into the next.
    let all_obs: Vec<_> = match store.list_memories_by_user_and_namespace(user_id, DIRECTIVE_NAMESPACE, 400, 0) {
        Ok(mems) => mems
            .into_iter()
            .filter(|m| !(m.namespace == DIRECTIVE_NAMESPACE && m.key == DIRECTIVE_KEY))
            .collect(),
        Err(e) => {
            debug!(error = %e, "failed to load style observations");
            return None;
        }
    };

    if all_obs.len() < MIN_OBSERVATIONS {
        debug!(
            count = all_obs.len(),
            min = MIN_OBSERVATIONS,
            "not enough style observations for synthesis"
        );
        return None;
    }

    // Filter by decay: drop expired low-confidence observations
    let now = chrono::Utc::now().timestamp();
    let mut scored: Vec<(String, String, i64, f64)> = Vec::new(); // (key, value, reinforced_count, confidence)

    for mem in &all_obs {
        let meta: serde_json::Value = mem
            .metadata
            .as_deref()
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default();

        let reinforced_count = meta
            .get("reinforced_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let confidence = meta
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.75);
        let first_observed = meta
            .get("first_observed")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // Lifespan = reinforced_count * 14 days
        let lifespan_secs = reinforced_count * LIFESPAN_DAYS_PER_REINFORCEMENT * 86400;
        let age_secs = now - first_observed;

        // Drop expired low-confidence observations
        if first_observed > 0 && age_secs > lifespan_secs && confidence < 0.7 {
            continue;
        }

        scored.push((
            mem.key.clone(),
            mem.value.clone(),
            reinforced_count,
            confidence,
        ));
    }

    if scored.len() < MIN_OBSERVATIONS {
        return None;
    }

    // Sort by reinforced_count DESC, take top MAX_OBSERVATIONS
    scored.sort_by(|a, b| b.2.cmp(&a.2));
    scored.truncate(MAX_OBSERVATIONS);

    // Build LLM prompt
    let observations_text: Vec<String> = scored
        .iter()
        .map(|(key, value, count, conf)| {
            format!(
                "- {} = {} (reinforced {}x, confidence {:.2})",
                key, value, count, conf
            )
        })
        .collect();

    let input_hash = hex::encode(sha2::Sha256::digest(observations_text.join("\n").as_bytes()));
    let written_from = store
        .get_memory_by_key_and_user(DIRECTIVE_NAMESPACE, DIRECTIVE_KEY, user_id)
        .ok()
        .flatten()
        .and_then(|m| m.metadata)
        .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
        .and_then(|m| m.get("input_hash").and_then(|h| h.as_str()).map(str::to_string));
    if written_from.as_deref() == Some(input_hash.as_str()) {
        debug!("personality observations unchanged: the directive stands");
        return None;
    }

    let prompt = format!(
        "Based on these observed communication style patterns, write a concise personality directive (one paragraph, 3-5 sentences, second person \"you\").\n\
         The directive should guide an AI employee on how to communicate with this user.\n\
         Focus on concrete behaviors, not abstract descriptions.\n\n\
         Observations:\n{}\n\n\
         Write ONLY the directive paragraph, no preamble or explanation.",
        observations_text.join("\n")
    );

    let req = ai::ChatRequest {
        tool_credential: None,
        chat_id: String::new(),
        ask_channels: None,
        permission_mode: None,
        linked_context: None,
        tool_choice: Default::default(),
        messages: vec![ai::Message {
            role: "user".to_string(),
            content: prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 512,
        temperature: 0.3,
        system: "You are a personality synthesis engine. Produce concise, actionable directives."
            .to_string(),
        model: model.to_string(),
        enable_thinking: false,
        effort: None,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: ai::RequestTrace::new("personality"),
    };

    let mut rx = match provider.stream(&req).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!(error = %e, "personality synthesis LLM error");
            return None;
        }
    };

    let mut directive = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == StreamEventType::Text {
            directive.push_str(&event.text);
        }
    }

    let directive = directive.trim().to_string();
    if directive.is_empty() {
        return None;
    }

    // Store as tacit/personality/directive
    let metadata = serde_json::json!({
        "synthesized_at": now,
        "observation_count": scored.len(),
        "confidence": 0.95,
        "input_hash": input_hash,
    })
    .to_string();

    if let Err(e) = store.upsert_memory(
        DIRECTIVE_NAMESPACE,
        DIRECTIVE_KEY,
        &directive,
        None,
        Some(&metadata),
        user_id,
    ) {
        warn!(error = %e, "failed to store personality directive");
        return None;
    }

    info!(
        observation_count = scored.len(),
        "personality directive synthesized"
    );
    Some(directive)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that writes a directive and counts how often it was asked.
    struct Writer(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl Provider for Writer {
        fn id(&self) -> &str {
            "writer"
        }
        async fn stream(&self, _req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tx.send(ai::StreamEvent::text(format!("Be brief, version {n}."))).await.unwrap();
            Ok(rx)
        }
    }

    /// The directive is written once per set of observations: unchanged
    /// observations make no call, a new one does, and the directive never
    /// counts as an observation of its own.
    #[tokio::test]
    async fn an_unchanged_observation_set_is_never_synthesized_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap();
        for i in 0..MIN_OBSERVATIONS {
            store.upsert_memory("tacit/personality/style", &format!("s{i}"), "short replies", None, None, "u").unwrap();
        }
        let writer = Writer(std::sync::atomic::AtomicUsize::new(0));
        let calls = || writer.0.load(std::sync::atomic::Ordering::SeqCst);
        assert!(synthesize_directive(&store, &writer, "m", "u").await.is_some());
        assert_eq!(calls(), 1);
        assert!(synthesize_directive(&store, &writer, "m", "u").await.is_none(), "unchanged: the directive stands");
        assert_eq!(calls(), 1, "and no call was made");
        store.upsert_memory("tacit/personality/style", "s-new", "no emoji", None, None, "u").unwrap();
        assert!(synthesize_directive(&store, &writer, "m", "u").await.is_some(), "a new observation: written again");
        assert_eq!(calls(), 2);
        let stored = store.get_memory_by_key_and_user(DIRECTIVE_NAMESPACE, DIRECTIVE_KEY, "u").unwrap().unwrap();
        assert_eq!(stored.value, "Be brief, version 1.");
    }

    #[test]
    fn test_min_observations_check() {
        // Can't test full synthesis without an LLM, but we can verify the threshold logic
        assert!(MIN_OBSERVATIONS == 5);
        assert!(MAX_OBSERVATIONS == 15);
    }

    #[test]
    fn test_lifespan_calculation() {
        let reinforced_count = 3i64;
        let lifespan_secs = reinforced_count * LIFESPAN_DAYS_PER_REINFORCEMENT * 86400;
        // 3 * 14 * 86400 = 42 days in seconds
        assert_eq!(lifespan_secs, 42 * 86400);
    }

    #[test]
    fn test_decay_filter_logic() {
        let now = chrono::Utc::now().timestamp();
        let first_observed = now - (60 * 86400); // 60 days ago
        let reinforced_count = 2i64; // lifespan = 28 days
        let confidence = 0.5;

        let lifespan_secs = reinforced_count * LIFESPAN_DAYS_PER_REINFORCEMENT * 86400;
        let age_secs = now - first_observed;

        // Should be expired: age (60 days) > lifespan (28 days) and low confidence
        assert!(age_secs > lifespan_secs && confidence < 0.7);
    }

    #[test]
    fn test_high_confidence_survives_expiry() {
        let now = chrono::Utc::now().timestamp();
        let first_observed = now - (60 * 86400); // 60 days ago
        let reinforced_count = 2i64; // lifespan = 28 days
        let confidence = 0.85; // high confidence

        let lifespan_secs = reinforced_count * LIFESPAN_DAYS_PER_REINFORCEMENT * 86400;
        let age_secs = now - first_observed;

        // Would be expired by age, but high confidence saves it
        assert!(age_secs > lifespan_secs);
        assert!(confidence >= 0.7); // survives the filter
    }

    #[test]
    fn test_well_reinforced_survives() {
        let now = chrono::Utc::now().timestamp();
        let first_observed = now - (60 * 86400); // 60 days ago
        let reinforced_count = 5i64; // lifespan = 70 days
        let _confidence = 0.5;

        let lifespan_secs = reinforced_count * LIFESPAN_DAYS_PER_REINFORCEMENT * 86400;
        let age_secs = now - first_observed;

        // Lifespan (70 days) > age (60 days), so survives
        assert!(age_secs <= lifespan_secs);
    }
}
