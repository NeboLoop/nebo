use std::collections::HashMap;

use crate::selector::ModelInfo;

/// Variant tokens — common model suffixes that affect scoring.
const VARIANT_TOKENS: &[&str] = &[
    "lightning", "preview", "mini", "fast", "turbo", "lite",
    "beta", "small", "nano", "instant", "pro", "thinking",
];

/// Fuzzy model matching engine with alias building.
pub struct FuzzyMatcher {
    aliases: HashMap<String, String>, // lowercase alias -> full model_id
}

impl FuzzyMatcher {
    /// Create a new matcher from model info and optional user aliases.
    pub fn new(
        provider_models: &HashMap<String, Vec<ModelInfo>>,
        user_aliases: &HashMap<String, String>,
        provider_credentials: &HashMap<String, bool>,
    ) -> Self {
        let mut f = Self {
            aliases: HashMap::new(),
        };
        f.build_aliases(provider_models, user_aliases, provider_credentials);
        f
    }

    fn build_aliases(
        &mut self,
        provider_models: &HashMap<String, Vec<ModelInfo>>,
        user_aliases: &HashMap<String, String>,
        provider_credentials: &HashMap<String, bool>,
    ) {
        // 1. User-configured aliases (highest priority)
        for (alias, model_id) in user_aliases {
            self.aliases.insert(alias.to_lowercase(), model_id.clone());
        }

        // Track kind mappings for step 3
        let mut kind_to_models: HashMap<String, Vec<String>> = HashMap::new();
        let mut kind_preferred: HashMap<String, String> = HashMap::new();
        let mut first_api_provider = String::new();

        // 2. Build aliases from provider models
        for (provider_name, models) in provider_models {
            if models.is_empty() {
                continue;
            }

            // Get first active model
            let first_model = models.iter().find(|m| m.active).map(|m| m.id.clone());
            let Some(first_model) = first_model else {
                continue;
            };

            let full_id = format!("{}/{}", provider_name, first_model);

            // Provider name as alias
            self.aliases.insert(provider_name.to_lowercase(), full_id.clone());

            // Track first API provider
            if first_api_provider.is_empty() {
                if provider_credentials.get(provider_name).copied().unwrap_or(false) {
                    first_api_provider = full_id.clone();
                }
            }

            // Add all model IDs, display names, and kind tags
            for m in models {
                if !m.active {
                    continue;
                }
                let m_full_id = format!("{}/{}", provider_name, m.id);
                self.aliases.insert(m.id.to_lowercase(), m_full_id.clone());
                self.aliases.insert(m_full_id.to_lowercase(), m_full_id.clone());
                if !m.display_name.is_empty() {
                    self.aliases.insert(m.display_name.to_lowercase(), m_full_id.clone());
                }

                // Short-form aliases from model ID parts
                let id_lower = m.id.to_lowercase();
                let parts: Vec<String> = id_lower
                    .split(|c: char| c == '-' || c == '_' || c == '.')
                    .filter(|p| p.len() >= 3 && !is_numeric(p) && *p != "claude" && *p != "gpt")
                    .map(|s| s.to_string())
                    .collect();
                for part in parts {
                    self.aliases.entry(part).or_insert_with(|| m_full_id.clone());
                }

                // Build kind mappings
                for kind in &m.kind {
                    let kind_lower = kind.to_lowercase();
                    kind_to_models.entry(kind_lower.clone()).or_default().push(m_full_id.clone());
                    if m.preferred {
                        kind_preferred.insert(kind_lower, m_full_id.clone());
                    }
                }
            }
        }

        // 3. Add kind tags as aliases
        for (kind, models) in &kind_to_models {
            if self.aliases.contains_key(kind) {
                continue;
            }
            if let Some(preferred) = kind_preferred.get(kind) {
                self.aliases.insert(kind.clone(), preferred.clone());
            } else if let Some(first) = models.first() {
                self.aliases.insert(kind.clone(), first.clone());
            }
        }

        // 4. Add "api" alias
        if !first_api_provider.is_empty() {
            self.aliases.entry("api".to_string()).or_insert(first_api_provider);
        }
    }

    /// Register an additional alias.
    pub fn add_alias(&mut self, alias: &str, model_id: &str) {
        self.aliases.insert(alias.to_lowercase(), model_id.to_string());
    }

    /// Resolve user input to a model ID. Returns None if no good match.
    pub fn resolve(&self, input: &str) -> Option<String> {
        let input = input.to_lowercase();
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        let normalized_input = normalize(input);
        let input_words: Vec<&str> = input.split_whitespace().collect();
        let input_variants = extract_variants(input);

        let mut best_model: Option<String> = None;
        let mut best_score = 0i32;
        let mut best_len = usize::MAX;

        for (alias, model_id) in &self.aliases {
            let score = score_match(input, &normalized_input, &input_words, &input_variants, alias, model_id);
            if score > 0 {
                if score > best_score || (score == best_score && model_id.len() < best_len) {
                    best_score = score;
                    best_model = Some(model_id.clone());
                    best_len = model_id.len();
                }
            }
        }

        // Minimum threshold
        if best_score < 50 {
            return None;
        }

        best_model
    }

    /// Returns formatted alias list for system prompt injection.
    pub fn get_aliases_text(&self) -> String {
        if self.aliases.is_empty() {
            return String::new();
        }

        // Dedupe by model ID — keep shortest alias per model
        let mut model_to_alias: HashMap<&str, &str> = HashMap::new();
        for (alias, model_id) in &self.aliases {
            // Skip very short aliases, full IDs, and numeric-only
            if alias.len() < 3 || alias.contains('/') || is_numeric(alias) {
                continue;
            }
            match model_to_alias.get(model_id.as_str()) {
                Some(existing) if alias.len() < existing.len() => {
                    model_to_alias.insert(model_id, alias);
                }
                None => {
                    model_to_alias.insert(model_id, alias);
                }
                _ => {}
            }
        }

        let mut lines: Vec<String> = model_to_alias
            .iter()
            .map(|(model_id, alias)| format!("- {}: {}", alias, model_id))
            .collect();
        lines.sort();
        lines.join("\n")
    }
}

/// Parse user input for model switching requests.
/// Returns the model name if user is requesting a switch, None otherwise.
pub fn parse_model_request(input: &str) -> Option<String> {
    let input = input.to_lowercase();
    let input = input.trim();

    let patterns = ["use ", "switch to ", "change to ", "try ", "with "];

    for pattern in &patterns {
        if let Some(idx) = input.find(pattern) {
            let mut remainder = &input[idx + pattern.len()..];
            // Strip trailing suffixes
            for suffix in &[" model", " please", " for this"] {
                if let Some(stripped) = remainder.strip_suffix(suffix) {
                    remainder = stripped;
                }
            }
            let remainder = remainder.trim();
            // Strip punctuation
            let cleaned: String = remainder.chars().filter(|c| !c.is_ascii_punctuation()).collect();
            let cleaned = cleaned.trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }

    None
}

/// Calculate match score between input and an alias.
fn score_match(
    input: &str,
    normalized_input: &str,
    input_words: &[&str],
    input_variants: &[String],
    alias: &str,
    model_id: &str,
) -> i32 {
    let mut score: i32 = 0;
    let alias_lower = alias.to_lowercase();
    let normalized_alias = normalize(alias);

    // Extract provider and model from model_id
    let (provider_lower, model_lower) = match model_id.split_once('/') {
        Some((p, m)) => (p.to_lowercase(), m.to_lowercase()),
        None => (String::new(), String::new()),
    };

    // 1. Exact match
    if input == alias_lower {
        score += 300;
    }

    // 2. Normalized exact match
    if normalized_input == normalized_alias {
        score += 250;
    }

    // 3. Prefix matching
    if input.starts_with(&alias_lower) {
        score += 150;
    }
    if alias_lower.starts_with(input) {
        score += 140;
    }

    // 4. Normalized prefix
    if normalized_input.starts_with(&normalized_alias) {
        score += 130;
    }
    if normalized_alias.starts_with(normalized_input) {
        score += 120;
    }

    // 5. Contains matching
    if input.contains(&alias_lower) {
        score += 100;
    }
    if alias_lower.contains(input) {
        score += 90;
    }

    // 6. Normalized contains
    if normalized_alias.len() >= 3 && normalized_input.contains(&normalized_alias) {
        score += 80;
    }
    if normalized_input.len() >= 3 && normalized_alias.contains(normalized_input) {
        score += 70;
    }

    // 7. Word matching
    for word in input_words {
        if word.len() < 3 {
            continue;
        }
        if *word == alias_lower {
            score += 120;
        } else if alias_lower.contains(word) {
            score += 60;
        } else if model_lower.contains(word) {
            score += 50;
        } else if provider_lower.contains(word) {
            score += 40;
        }
    }

    // 8. Levenshtein distance for typo tolerance
    if let Some(dist) = bounded_levenshtein(input, &alias_lower, 3) {
        score += (4 - dist as i32) * 50;
    }
    if let Some(dist) = bounded_levenshtein(normalized_input, &normalized_alias, 3) {
        score += (4 - dist as i32) * 40;
    }

    // 9. Variant token handling
    let alias_variants = extract_variants(&alias_lower);
    let model_variants = extract_variants(&model_lower);
    let mut all_model_variants: Vec<String> = alias_variants;
    all_model_variants.extend(model_variants);
    all_model_variants.sort();
    all_model_variants.dedup();

    if !input_variants.is_empty() {
        let match_count = input_variants.iter()
            .filter(|iv| all_model_variants.iter().any(|mv| iv.as_str() == mv.as_str()))
            .count();
        if match_count > 0 {
            score += match_count as i32 * 60;
        } else if !all_model_variants.is_empty() {
            score -= 30;
        }
    } else if !all_model_variants.is_empty() {
        score -= all_model_variants.len() as i32 * 15;
    }

    score
}

/// Remove dashes, dots, spaces, underscores for fuzzy comparison.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| *c != '-' && *c != '.' && *c != ' ' && *c != '_')
        .collect()
}

/// Check if string is all digits.
fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Extract variant tokens found in the string.
fn extract_variants(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    VARIANT_TOKENS.iter()
        .filter(|v| lower.contains(*v))
        .map(|v| v.to_string())
        .collect()
}

/// Levenshtein distance with early exit. Returns None if distance exceeds max_dist.
fn bounded_levenshtein(a: &str, b: &str, max_dist: usize) -> Option<usize> {
    if a == b {
        return Some(0);
    }
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let len_diff = if a.len() > b.len() { a.len() - b.len() } else { b.len() - a.len() };
    if len_diff > max_dist {
        return None;
    }

    let r1: Vec<char> = a.chars().collect();
    let r2: Vec<char> = b.chars().collect();
    let (len1, len2) = (r1.len(), r2.len());

    let mut prev = vec![0usize; len2 + 1];
    let mut curr = vec![0usize; len2 + 1];

    for j in 0..=len2 {
        prev[j] = j;
    }

    for i in 1..=len1 {
        curr[0] = i;
        let mut row_min = curr[0];

        for j in 1..=len2 {
            let cost = if r1[i - 1] == r2[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if curr[j] < row_min {
                row_min = curr[j];
            }
        }

        if row_min > max_dist {
            return None;
        }

        std::mem::swap(&mut prev, &mut curr);
    }

    let dist = prev[len2];
    if dist > max_dist {
        None
    } else {
        Some(dist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_models() -> HashMap<String, Vec<ModelInfo>> {
        let mut m = HashMap::new();
        m.insert("anthropic".to_string(), vec![
            ModelInfo {
                id: "claude-sonnet-4".to_string(),
                display_name: "Claude Sonnet 4".to_string(),
                context_window: 200_000,
                input_price: 3.0,
                output_price: 15.0,
                capabilities: vec!["general".to_string()],
                kind: vec!["smart".to_string()],
                preferred: true,
                active: true,
            },
            ModelInfo {
                id: "claude-opus-4".to_string(),
                display_name: "Claude Opus 4".to_string(),
                context_window: 200_000,
                input_price: 15.0,
                output_price: 75.0,
                capabilities: vec!["thinking".to_string()],
                kind: vec!["reasoning".to_string()],
                preferred: false,
                active: true,
            },
        ]);
        m.insert("openai".to_string(), vec![
            ModelInfo {
                id: "gpt-4o".to_string(),
                display_name: "GPT-4o".to_string(),
                context_window: 128_000,
                input_price: 2.5,
                output_price: 10.0,
                capabilities: vec!["general".to_string()],
                kind: vec!["fast".to_string()],
                preferred: false,
                active: true,
            },
        ]);
        m
    }

    fn test_credentials() -> HashMap<String, bool> {
        let mut c = HashMap::new();
        c.insert("anthropic".to_string(), true);
        c.insert("openai".to_string(), true);
        c
    }

    #[test]
    fn test_exact_alias_resolution() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        // Provider name
        let result = matcher.resolve("anthropic");
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("anthropic/"));
    }

    #[test]
    fn test_model_id_resolution() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        let result = matcher.resolve("claude-sonnet-4");
        assert_eq!(result, Some("anthropic/claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_display_name_resolution() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        let result = matcher.resolve("Claude Sonnet 4");
        assert_eq!(result, Some("anthropic/claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_short_alias_resolution() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        // "sonnet" should resolve
        let result = matcher.resolve("sonnet");
        assert_eq!(result, Some("anthropic/claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_typo_tolerance() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        // "sonet" (typo) should still match "sonnet"
        let result = matcher.resolve("sonet");
        assert!(result.is_some());
        assert!(result.unwrap().contains("sonnet"));
    }

    #[test]
    fn test_user_aliases() {
        let mut user_aliases = HashMap::new();
        user_aliases.insert("mymodel".to_string(), "anthropic/claude-opus-4".to_string());
        let matcher = FuzzyMatcher::new(&test_models(), &user_aliases, &test_credentials());
        let result = matcher.resolve("mymodel");
        assert_eq!(result, Some("anthropic/claude-opus-4".to_string()));
    }

    #[test]
    fn test_no_match() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        let result = matcher.resolve("completely-nonexistent-model-xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_model_request() {
        assert_eq!(parse_model_request("use sonnet"), Some("sonnet".to_string()));
        assert_eq!(parse_model_request("switch to opus please"), Some("opus".to_string()));
        assert_eq!(parse_model_request("change to gpt-4o model"), Some("gpt4o".to_string()));
        assert_eq!(parse_model_request("try claude"), Some("claude".to_string()));
        assert_eq!(parse_model_request("hello world"), None);
    }

    #[test]
    fn test_get_aliases_text() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        let text = matcher.get_aliases_text();
        assert!(!text.is_empty());
        // Should contain formatted lines
        assert!(text.contains(": "));
    }

    #[test]
    fn test_bounded_levenshtein() {
        assert_eq!(bounded_levenshtein("kitten", "sitting", 3), Some(3));
        assert_eq!(bounded_levenshtein("abc", "abc", 3), Some(0));
        assert_eq!(bounded_levenshtein("abc", "xyz", 2), None);
        assert_eq!(bounded_levenshtein("", "abc", 3), None);
    }

    #[test]
    fn test_variant_matching() {
        let matcher = FuzzyMatcher::new(&test_models(), &HashMap::new(), &test_credentials());
        // "fast" is a kind tag for gpt-4o
        let result = matcher.resolve("fast");
        assert!(result.is_some());
    }
}
