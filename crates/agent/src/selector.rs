use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use config::ModelsConfig;
use db::models::ChatMessage;

use crate::fuzzy::FuzzyMatcher;

/// Task types for model routing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskType {
    Vision,
    Audio,
    Reasoning,
    Code,
    General,
}

impl TaskType {
    pub fn as_str(&self) -> &str {
        match self {
            TaskType::Vision => "vision",
            TaskType::Audio => "audio",
            TaskType::Reasoning => "reasoning",
            TaskType::Code => "code",
            TaskType::General => "general",
        }
    }
}

/// Per-model failure tracking with exponential backoff.
struct CooldownState {
    failure_count: u32,
    cooldown_until: Instant,
}

/// Model information for routing decisions.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub context_window: i32,
    pub input_price: f64,
    pub output_price: f64,
    pub capabilities: Vec<String>,
    pub kind: Vec<String>,
    pub preferred: bool,
    pub active: bool,
}

/// Model routing configuration.
#[derive(Debug, Clone, Default)]
pub struct ModelRoutingConfig {
    /// Primary model for each task type.
    pub task_routing: HashMap<String, String>,
    /// Fallback models per task type.
    pub task_fallbacks: HashMap<String, Vec<String>>,
    /// Default primary model.
    pub default_model: String,
    /// Provider -> list of models.
    pub provider_models: HashMap<String, Vec<ModelInfo>>,
    /// Provider credentials (provider_id -> has_api_key).
    pub provider_credentials: HashMap<String, bool>,
}

impl ModelRoutingConfig {
    /// Build a routing config from the models.yaml catalog and active provider IDs.
    pub fn from_models_config(
        models_cfg: &ModelsConfig,
        active_provider_ids: &[String],
    ) -> Self {
        let mut provider_models: HashMap<String, Vec<ModelInfo>> = HashMap::new();
        let mut provider_credentials: HashMap<String, bool> = HashMap::new();

        for (provider_name, models) in &models_cfg.providers {
            let has_creds = active_provider_ids.iter().any(|id| id == provider_name);
            provider_credentials.insert(provider_name.clone(), has_creds);

            let infos: Vec<ModelInfo> = models
                .iter()
                .map(|m| {
                    let (input_price, output_price) = match &m.pricing {
                        Some(p) => (p.input, p.output),
                        None => (0.0, 0.0),
                    };
                    ModelInfo {
                        id: m.id.clone(),
                        display_name: m.display_name.clone(),
                        context_window: m.context_window as i32,
                        input_price,
                        output_price,
                        capabilities: m.capabilities.clone(),
                        kind: m.kind.clone(),
                        preferred: m.preferred,
                        active: m.is_active(),
                    }
                })
                .collect();
            provider_models.insert(provider_name.clone(), infos);
        }

        // Build task routing from config
        let mut task_routing = HashMap::new();
        let mut task_fallbacks = HashMap::new();
        if let Some(ref tr) = models_cfg.task_routing {
            if !tr.vision.is_empty() {
                task_routing.insert("vision".to_string(), tr.vision.clone());
            }
            if !tr.audio.is_empty() {
                task_routing.insert("audio".to_string(), tr.audio.clone());
            }
            if !tr.reasoning.is_empty() {
                task_routing.insert("reasoning".to_string(), tr.reasoning.clone());
            }
            if !tr.code.is_empty() {
                task_routing.insert("code".to_string(), tr.code.clone());
            }
            if !tr.general.is_empty() {
                task_routing.insert("general".to_string(), tr.general.clone());
            }
            task_fallbacks = tr.fallbacks.clone();
        }

        // Default model from config
        let default_model = models_cfg
            .defaults
            .as_ref()
            .map(|d| d.primary.clone())
            .unwrap_or_default();

        ModelRoutingConfig {
            task_routing,
            task_fallbacks,
            default_model,
            provider_models,
            provider_credentials,
        }
    }
}

/// Thread-safe model router with task classification and cooldown tracking.
pub struct ModelSelector {
    config: ModelRoutingConfig,
    cooldowns: RwLock<HashMap<String, CooldownState>>,
    excluded: RwLock<HashMap<String, bool>>,
    fuzzy: RwLock<Option<FuzzyMatcher>>,
    /// Provider IDs that are actually loaded (have running Provider instances).
    loaded_providers: RwLock<Vec<String>>,
}

impl ModelSelector {
    pub fn new(config: ModelRoutingConfig) -> Self {
        let fuzzy = FuzzyMatcher::new(
            &config.provider_models,
            &HashMap::new(),
            &config.provider_credentials,
        );
        Self {
            config,
            cooldowns: RwLock::new(HashMap::new()),
            excluded: RwLock::new(HashMap::new()),
            fuzzy: RwLock::new(Some(fuzzy)),
            loaded_providers: RwLock::new(Vec::new()),
        }
    }

    /// Update the set of loaded provider IDs (providers that have running instances).
    pub fn set_loaded_providers(&self, provider_ids: Vec<String>) {
        let mut lock = self.loaded_providers.write().unwrap();
        *lock = provider_ids;
    }

    /// Resolve a fuzzy model name (e.g. "sonnet", "opus") to a full model ID.
    pub fn resolve_fuzzy(&self, input: &str) -> Option<String> {
        let lock = self.fuzzy.read().unwrap();
        lock.as_ref().and_then(|f| f.resolve(input))
    }

    /// Get formatted model aliases text for system prompt injection.
    pub fn get_aliases_text(&self) -> String {
        let lock = self.fuzzy.read().unwrap();
        lock.as_ref().map(|f| f.get_aliases_text()).unwrap_or_default()
    }

    /// Rebuild the fuzzy matcher (e.g., after provider reload).
    pub fn rebuild_fuzzy(&self, user_aliases: &HashMap<String, String>) {
        let new_fuzzy = FuzzyMatcher::new(
            &self.config.provider_models,
            user_aliases,
            &self.config.provider_credentials,
        );
        let mut lock = self.fuzzy.write().unwrap();
        *lock = Some(new_fuzzy);
    }

    /// Select the best model for the given messages.
    pub fn select(&self, messages: &[ChatMessage]) -> String {
        self.select_with_exclusions(messages, &[])
    }

    /// Select model, excluding specified model IDs.
    pub fn select_with_exclusions(&self, messages: &[ChatMessage], exclude: &[String]) -> String {
        let task = self.classify_task(messages);
        self.select_for_task(&task, exclude)
    }

    /// Mark a model as failed with exponential backoff cooldown.
    pub fn mark_failed(&self, model_id: &str) {
        let mut cooldowns = self.cooldowns.write().unwrap();
        let entry = cooldowns.entry(model_id.to_string()).or_insert(CooldownState {
            failure_count: 0,
            cooldown_until: Instant::now(),
        });
        entry.failure_count += 1;
        // Exponential backoff: 5s, 10s, 20s, 40s... capped at 1 hour
        let backoff_secs = std::cmp::min(5 * 2u64.pow(entry.failure_count.saturating_sub(1)), 3600);
        entry.cooldown_until = Instant::now() + Duration::from_secs(backoff_secs);

        self.excluded
            .write()
            .unwrap()
            .insert(model_id.to_string(), true);
    }

    /// Clear all failures and cooldowns.
    pub fn clear_failed(&self) {
        self.cooldowns.write().unwrap().clear();
        self.excluded.write().unwrap().clear();
    }

    /// Check remaining cooldown for a model.
    pub fn get_cooldown_remaining(&self, model_id: &str) -> Duration {
        let cooldowns = self.cooldowns.read().unwrap();
        match cooldowns.get(model_id) {
            Some(state) => {
                let now = Instant::now();
                if state.cooldown_until > now {
                    state.cooldown_until - now
                } else {
                    Duration::ZERO
                }
            }
            None => Duration::ZERO,
        }
    }

    /// Get model info by "provider/model" ID.
    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let (provider_id, model_name) = parse_model_id(model_id);
        if let Some(models) = self.config.provider_models.get(provider_id) {
            models.iter().find(|m| m.id == model_name).cloned()
        } else {
            None
        }
    }

    /// Check if a model supports extended thinking/reasoning.
    pub fn supports_thinking(&self, model_id: &str) -> bool {
        if let Some(info) = self.get_model_info(model_id) {
            let thinking_caps = ["thinking", "reasoning", "extended_thinking"];
            if info.capabilities.iter().any(|c| thinking_caps.contains(&c.as_str())) {
                return true;
            }
        }

        // Name-based fallback
        let lower = model_id.to_lowercase();
        lower.contains("opus") || lower.contains("o1") || lower.contains("o3")
    }

    /// Get the cheapest available model.
    pub fn get_cheapest_model(&self) -> String {
        let mut cheapest: Option<(String, f64)> = None;

        for (provider_id, models) in &self.config.provider_models {
            // Skip providers without credentials
            if !self.config.provider_credentials.get(provider_id).copied().unwrap_or(false) {
                continue;
            }
            for model in models {
                if !model.active {
                    continue;
                }
                let cost = model.input_price + model.output_price * 2.0;
                let model_id = format!("{}/{}", provider_id, model.id);
                if cheapest.is_none() || cost < cheapest.as_ref().unwrap().1 {
                    cheapest = Some((model_id, cost));
                }
            }
        }

        if let Some((id, _)) = cheapest {
            return id;
        }

        // Fallback: find any model with "cheap" or "fast" kind
        for (provider_id, models) in &self.config.provider_models {
            for model in models {
                if model.active && (model.kind.contains(&"cheap".to_string()) || model.kind.contains(&"fast".to_string())) {
                    return format!("{}/{}", provider_id, model.id);
                }
            }
        }

        self.config.default_model.clone()
    }

    /// Classify task type from messages.
    pub fn classify_task(&self, messages: &[ChatMessage]) -> TaskType {
        // Get last user message for keyword analysis
        let last_user = messages
            .iter()
            .rev()
            .find(|m| m.role == "user");

        let content = match last_user {
            Some(m) => m.content.to_lowercase(),
            None => return TaskType::General,
        };

        // Check for vision content (image data in the message)
        if content.contains("data:image/") || content.contains("\"type\":\"image\"") || content.contains("\"type\": \"image\"") {
            return TaskType::Vision;
        }

        // Check for audio content
        if content.contains("data:audio/") || content.contains("\"type\":\"audio\"") {
            return TaskType::Audio;
        }

        // Reasoning keywords
        let reasoning = [
            "think through", "analyze", "prove", "step by step",
            "mathematical proof", "logical reasoning", "derive",
            "theorem", "hypothesis", "contradict", "paradox",
            "evaluate the", "compare and contrast", "pros and cons",
            "trade-offs", "implications",
        ];
        if reasoning.iter().any(|kw| content.contains(kw)) {
            return TaskType::Reasoning;
        }

        // Code keywords
        let code = [
            "code", "function", "implement", "refactor", "debug",
            "python", "javascript", "typescript", "react", "rust",
            "golang", "java", "swift", "kotlin", "sql", "api",
            "endpoint", "database", "algorithm", "compile",
            "syntax", "variable", "class",
        ];
        if code.iter().any(|kw| content.contains(kw)) {
            return TaskType::Code;
        }

        TaskType::General
    }

    fn select_for_task(&self, task: &TaskType, exclude: &[String]) -> String {
        let loaded = self.loaded_providers.read().unwrap();
        let is_usable = |model_id: &str| -> bool {
            if exclude.contains(&model_id.to_string()) {
                return false;
            }
            // Check if the model's provider is actually loaded
            if !loaded.is_empty() {
                let (provider_id, _) = parse_model_id(model_id);
                if !provider_id.is_empty() && !loaded.iter().any(|p| p == provider_id) {
                    return false;
                }
            }
            if self.excluded.read().unwrap().contains_key(model_id) {
                if self.get_cooldown_remaining(model_id) > Duration::ZERO {
                    return false;
                }
            }
            true
        };

        // Try task-specific routing
        let task_key = task.as_str();
        if let Some(primary) = self.config.task_routing.get(task_key) {
            if is_usable(primary) {
                return primary.clone();
            }
        }

        // Try fallbacks for this task type
        if let Some(fallbacks) = self.config.task_fallbacks.get(task_key) {
            for fb in fallbacks {
                if is_usable(fb) {
                    return fb.clone();
                }
            }
        }

        // Fall back to general routing
        if task_key != "general" {
            if let Some(general) = self.config.task_routing.get("general") {
                if is_usable(general) {
                    return general.clone();
                }
            }
        }

        // Final fallback: default model
        if is_usable(&self.config.default_model) {
            return self.config.default_model.clone();
        }

        // Last resort: any non-gateway available model
        for (provider_id, models) in &self.config.provider_models {
            if provider_id == "janus" {
                continue; // Skip gateway — prefer CLI or direct API
            }
            for model in models {
                let id = format!("{}/{}", provider_id, model.id);
                if model.active && is_usable(&id) {
                    return id;
                }
            }
        }

        // If CLI providers are loaded, return empty so the runner uses
        // index 0 (CLI provider, after reordering) instead of Janus.
        let has_cli = loaded.iter().any(|p| {
            p == "claude-code" || p == "codex-cli" || p == "gemini-cli"
        });
        if has_cli {
            return String::new();
        }

        // True last resort: gateway model
        for (provider_id, models) in &self.config.provider_models {
            for model in models {
                let id = format!("{}/{}", provider_id, model.id);
                if model.active && is_usable(&id) {
                    return id;
                }
            }
        }

        self.config.default_model.clone()
    }
}

/// Parse "provider/model" into (provider, model). Returns ("", model_id) if no slash.
pub fn parse_model_id(model_id: &str) -> (&str, &str) {
    match model_id.split_once('/') {
        Some((provider, model)) => (provider, model),
        None => ("", model_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_id() {
        let (p, m) = parse_model_id("anthropic/claude-sonnet-4-5");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-sonnet-4-5");

        let (p, m) = parse_model_id("gpt-4o");
        assert_eq!(p, "");
        assert_eq!(m, "gpt-4o");
    }

    #[test]
    fn test_task_classification() {
        let selector = ModelSelector::new(ModelRoutingConfig::default());

        let msg = ChatMessage {
            id: "1".into(),
            chat_id: "c".into(),
            role: "user".into(),
            content: "Can you implement a function that sorts an array?".into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        };
        assert_eq!(selector.classify_task(&[msg]).as_str(), "code");
    }

    #[test]
    fn test_cooldown_backoff() {
        let selector = ModelSelector::new(ModelRoutingConfig::default());
        selector.mark_failed("anthropic/claude-sonnet-4-5");
        assert!(selector.get_cooldown_remaining("anthropic/claude-sonnet-4-5") > Duration::ZERO);
        selector.clear_failed();
        assert_eq!(selector.get_cooldown_remaining("anthropic/claude-sonnet-4-5"), Duration::ZERO);
    }

    #[test]
    fn test_from_models_config() {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            vec![config::models::ModelDef {
                id: "claude-sonnet-4-20250514".to_string(),
                display_name: "Claude Sonnet 4".to_string(),
                context_window: 200000,
                pricing: Some(config::models::ModelPricing {
                    input: 3.0,
                    output: 15.0,
                    cached_input: 0.3,
                }),
                capabilities: vec!["tool_use".into(), "vision".into()],
                kind: vec!["smart".into()],
                preferred: true,
                active: Some(true),
            }],
        );
        let models_cfg = config::ModelsConfig {
            version: "1.0".into(),
            defaults: Some(config::models::Defaults {
                primary: "anthropic/claude-sonnet-4-20250514".into(),
                fallbacks: vec![],
            }),
            task_routing: None,
            lane_routing: None,
            aliases: vec![],
            providers,
            cli_providers: vec![],
        };

        let routing = ModelRoutingConfig::from_models_config(&models_cfg, &["anthropic".into()]);
        assert_eq!(routing.default_model, "anthropic/claude-sonnet-4-20250514");
        assert!(routing.provider_credentials.get("anthropic").copied().unwrap_or(false));
        assert_eq!(routing.provider_models.get("anthropic").unwrap().len(), 1);
    }

    #[test]
    fn test_loaded_providers_filter() {
        let mut provider_models = HashMap::new();
        provider_models.insert(
            "anthropic".to_string(),
            vec![ModelInfo {
                id: "claude-sonnet-4".to_string(),
                display_name: "Sonnet".to_string(),
                context_window: 200000,
                input_price: 3.0,
                output_price: 15.0,
                capabilities: vec![],
                kind: vec![],
                preferred: true,
                active: true,
            }],
        );
        provider_models.insert(
            "openai".to_string(),
            vec![ModelInfo {
                id: "gpt-4o".to_string(),
                display_name: "GPT-4o".to_string(),
                context_window: 128000,
                input_price: 5.0,
                output_price: 15.0,
                capabilities: vec![],
                kind: vec![],
                preferred: false,
                active: true,
            }],
        );

        let mut creds = HashMap::new();
        creds.insert("anthropic".into(), true);
        creds.insert("openai".into(), true);

        let config = ModelRoutingConfig {
            task_routing: HashMap::new(),
            task_fallbacks: HashMap::new(),
            default_model: "anthropic/claude-sonnet-4".into(),
            provider_models,
            provider_credentials: creds,
        };

        let selector = ModelSelector::new(config);
        // Only load anthropic — openai models should be filtered out
        selector.set_loaded_providers(vec!["anthropic".into()]);

        let msg = ChatMessage {
            id: "1".into(),
            chat_id: "c".into(),
            role: "user".into(),
            content: "hello".into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        };

        let selected = selector.select(&[msg]);
        // Should pick an anthropic model since openai is not loaded
        assert!(selected.contains("anthropic"), "Expected anthropic model, got: {}", selected);
    }

    #[test]
    fn test_cli_preferred_over_janus() {
        // When only CLI + Janus are loaded (no direct API keys), the selector
        // should return empty string so the runner defers to index 0 (CLI)
        // instead of selecting a Janus model that burns Nebo credits.
        let mut provider_models = HashMap::new();
        provider_models.insert(
            "janus".to_string(),
            vec![ModelInfo {
                id: "nebo-1".to_string(),
                display_name: "Nebo 1".to_string(),
                context_window: 200000,
                input_price: 0.0,
                output_price: 0.0,
                capabilities: vec![],
                kind: vec![],
                preferred: true,
                active: true,
            }],
        );
        provider_models.insert(
            "anthropic".to_string(),
            vec![ModelInfo {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Sonnet".to_string(),
                context_window: 200000,
                input_price: 3.0,
                output_price: 15.0,
                capabilities: vec![],
                kind: vec![],
                preferred: true,
                active: true,
            }],
        );

        let mut creds = HashMap::new();
        creds.insert("janus".into(), true);
        creds.insert("anthropic".into(), false); // No API key

        let config = ModelRoutingConfig {
            task_routing: HashMap::new(),
            task_fallbacks: HashMap::new(),
            default_model: "anthropic/claude-sonnet-4-5".into(),
            provider_models,
            provider_credentials: creds,
        };

        let selector = ModelSelector::new(config);
        // Only janus + CLI loaded — no direct anthropic provider
        selector.set_loaded_providers(vec!["claude-code".into(), "janus".into()]);

        let msg = ChatMessage {
            id: "1".into(),
            chat_id: "c".into(),
            role: "user".into(),
            content: "hello".into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        };

        let selected = selector.select(&[msg]);
        // Should return empty (defer to runner index 0 = CLI), NOT "janus/nebo-1"
        assert!(
            selected.is_empty(),
            "Expected empty string (defer to CLI), got: {}",
            selected
        );
    }
}
