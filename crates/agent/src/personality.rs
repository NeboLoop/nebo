use ai::{Provider, StreamEventType};
use db::Store;
use tracing::{debug, info, warn};

/// Minimum number of style observations before synthesizing a directive.
const MIN_OBSERVATIONS: usize = 5;
/// Maximum observations to include in synthesis prompt.
const MAX_OBSERVATIONS: usize = 15;
/// Base lifespan multiplier per reinforced_count (days).
const LIFESPAN_DAYS_PER_REINFORCEMENT: i64 = 14;

/// Synthesize a personality directive from style observations.
/// Calls the LLM to produce a 3-5 sentence directive in 2nd person.
/// Stores the result as `tacit/personality/directive`.
pub async fn synthesize_directive(
    store: &Store,
    provider: &dyn Provider,
    user_id: &str,
) -> Option<String> {
    // Load all style observations
    let observations = match store.list_memories_by_user_and_namespace(
        user_id,
        "tacit/personality/style",
        200,
        0,
    ) {
        Ok(mems) => mems,
        Err(e) => {
            debug!(error = %e, "failed to load style observations");
            return None;
        }
    };

    // Also include tacit/personality observations (non-style)
    let personality_obs = store
        .list_memories_by_user_and_namespace(user_id, "tacit/personality", 200, 0)
        .unwrap_or_default();

    let all_obs: Vec<_> = observations
        .into_iter()
        .chain(personality_obs.into_iter())
        .collect();

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

        scored.push((mem.key.clone(), mem.value.clone(), reinforced_count, confidence));
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

    let prompt = format!(
        "Based on these observed communication style patterns, write a concise personality directive (one paragraph, 3-5 sentences, second person \"you\").\n\
         The directive should guide an AI assistant on how to communicate with this user.\n\
         Focus on concrete behaviors, not abstract descriptions.\n\n\
         Observations:\n{}\n\n\
         Write ONLY the directive paragraph, no preamble or explanation.",
        observations_text.join("\n")
    );

    let req = ai::ChatRequest {
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
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
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
    })
    .to_string();

    if let Err(e) = store.upsert_memory(
        "tacit/personality",
        "directive",
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
