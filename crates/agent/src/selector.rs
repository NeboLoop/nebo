//! Which model a turn is sent to. The harness does not pick or roam models:
//! Janus is the router. A turn runs on the model it was given (the owner's
//! pick, the job's or a helper's speed), else the configured default, and
//! any discrepancy (a name nobody knows, a provider that isn't loaded, a
//! model that doesn't chat) sends [`DEFAULT_CHAT_MODEL`]. A failed call is
//! retried on the same model (`model_call`): a model changes only for an
//! explicitly configured fallback, and Nebo configures none.

use std::collections::HashMap;
use std::sync::RwLock;

use config::ModelsConfig;
use tracing::warn;

use crate::fuzzy::FuzzyMatcher;

/// The model every discrepancy resolves to: the gateway's default chat
/// model, which Janus routes.
pub const DEFAULT_CHAT_MODEL: &str = "janus/nebo-1";

/// The window assumed for a model whose own is not known: the 200k every
/// Janus pool reports, and the common window of current chat models.
pub const DEFAULT_CONTEXT_WINDOW: usize = 200_000;

/// Capabilities or kinds that mark a model as not for chat.
const NOT_CHAT: &[&str] = &[
    "embeddings",
    "embedding",
    "embed",
    "audio",
    "transcription",
    "speech",
    "tts",
    "image_generation",
    "image-generation",
    "rerank",
];

/// The CLI providers: their models are the CLI's own names, not catalog rows.
const CLI_PROVIDERS: &[&str] = &["claude-code", "codex-cli", "gemini-cli"];

/// Model information for routing decisions.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub context_window: i32,
    pub input_price: f64,
    pub output_price: f64,
    /// $/M for cache reads. Distinct from input_price on purpose: billing
    /// cache reads at the input rate would overbill every long conversation,
    /// which is most of them.
    pub cached_input_price: f64,
    pub capabilities: Vec<String>,
    pub kind: Vec<String>,
    pub preferred: bool,
    pub active: bool,
}

/// Whether a model answers a conversation, from its id, capabilities and
/// kinds. An embedding, audio or image model never does: it is used only by
/// its own code path, and never offered where a chat model is chosen.
pub fn is_chat_model(id: &str, capabilities: &[String], kind: &[String]) -> bool {
    !id.to_ascii_lowercase().contains("embed")
        && !capabilities
            .iter()
            .chain(kind)
            .any(|c| NOT_CHAT.contains(&c.to_ascii_lowercase().as_str()))
}

impl ModelInfo {
    /// Whether this model answers a conversation ([`is_chat_model`]).
    pub fn chats(&self) -> bool {
        is_chat_model(&self.id, &self.capabilities, &self.kind)
    }
}

/// Only the chat models: what a name or a speed may resolve to.
fn chat_models(models: &HashMap<String, Vec<ModelInfo>>) -> HashMap<String, Vec<ModelInfo>> {
    models
        .iter()
        .map(|(p, list)| (p.clone(), list.iter().filter(|m| m.chats()).cloned().collect()))
        .collect()
}

/// Model routing configuration.
#[derive(Debug, Clone, Default)]
pub struct ModelRoutingConfig {
    /// The configured routes: "general" (a turn with no chosen model) and
    /// "aux" (background work).
    pub task_routing: HashMap<String, String>,
    /// Default primary model.
    pub default_model: String,
    /// Provider -> list of models.
    pub provider_models: HashMap<String, Vec<ModelInfo>>,
    /// Provider credentials (provider_id -> has_api_key).
    pub provider_credentials: HashMap<String, bool>,
}

impl ModelRoutingConfig {
    /// Build a routing config from the models.yaml catalog and active provider IDs.
    /// `model_overrides` maps "provider/model_id" → is_active from the DB,
    /// overriding the yaml catalog defaults so the selector respects user toggles.
    pub fn from_models_config(
        models_cfg: &ModelsConfig,
        active_provider_ids: &[String],
        model_overrides: &HashMap<String, bool>,
    ) -> Self {
        let mut provider_models: HashMap<String, Vec<ModelInfo>> = HashMap::new();
        let mut provider_credentials: HashMap<String, bool> = HashMap::new();

        for (provider_name, models) in &models_cfg.providers {
            let has_creds = active_provider_ids.iter().any(|id| id == provider_name);
            provider_credentials.insert(provider_name.clone(), has_creds);

            let infos: Vec<ModelInfo> = models
                .iter()
                .map(|m| {
                    let (input_price, output_price, cached_input_price) = match &m.pricing {
                        Some(p) => (p.input, p.output, p.cached_input),
                        None => (0.0, 0.0, 0.0),
                    };
                    let override_key = format!("{}/{}", provider_name, m.id);
                    ModelInfo {
                        id: m.id.clone(),
                        display_name: m.display_name.clone(),
                        context_window: m.context_window as i32,
                        input_price,
                        output_price,
                        cached_input_price,
                        capabilities: m.capabilities.clone(),
                        kind: m.kind.clone(),
                        preferred: m.preferred,
                        active: model_overrides
                            .get(&override_key)
                            .copied()
                            .unwrap_or(m.is_active()),
                    }
                })
                .collect();
            provider_models.insert(provider_name.clone(), infos);
        }

        let mut task_routing = HashMap::new();
        if let Some(tr) = models_cfg.task_routing.as_ref() {
            for (route, model) in [("general", &tr.general), ("aux", &tr.aux)] {
                if !model.is_empty() {
                    task_routing.insert(route.to_string(), model.clone());
                }
            }
        }

        // Default model from config
        let default_model = models_cfg
            .defaults
            .as_ref()
            .map(|d| d.primary.clone())
            .unwrap_or_default();

        ModelRoutingConfig {
            task_routing,
            default_model,
            provider_models,
            provider_credentials,
        }
    }
}

/// Thread-safe model resolution.
pub struct ModelSelector {
    config: ModelRoutingConfig,
    fuzzy: RwLock<Option<FuzzyMatcher>>,
    /// Provider IDs that are actually loaded (have running Provider instances).
    loaded_providers: RwLock<Vec<String>>,
    /// Models discovered at runtime (e.g., Ollama), not in the static yaml catalog.
    runtime_models: RwLock<HashMap<String, Vec<ModelInfo>>>,
}

impl ModelSelector {
    pub fn new(config: ModelRoutingConfig) -> Self {
        let fuzzy = FuzzyMatcher::new(
            &chat_models(&config.provider_models),
            &HashMap::new(),
            &config.provider_credentials,
        );
        Self {
            config,
            fuzzy: RwLock::new(Some(fuzzy)),
            loaded_providers: RwLock::new(Vec::new()),
            runtime_models: RwLock::new(HashMap::new()),
        }
    }

    /// Update the set of loaded provider IDs (providers that have running instances).
    pub fn set_loaded_providers(&self, provider_ids: Vec<String>) {
        let mut lock = self.loaded_providers.write().unwrap();
        *lock = provider_ids;
    }

    /// Resolve a fuzzy model name (e.g. "sonnet", "opus") to a full model ID.
    /// A linked agent's id (`linked/<bot>/<agent>`) is exact: it resolves to
    /// itself, and a malformed one to nothing — never scored against the
    /// model aliases, which would hand the employee another brain.
    pub fn resolve_fuzzy(&self, input: &str) -> Option<String> {
        if parse_model_id(input).0 == ai::providers::linked::ID {
            return ai::LinkedProvider::target(input).map(|_| input.to_string());
        }
        let lock = self.fuzzy.read().unwrap();
        lock.as_ref().and_then(|f| f.resolve(input))
    }

    /// Get formatted model aliases text for system prompt injection.
    pub fn get_aliases_text(&self) -> String {
        let lock = self.fuzzy.read().unwrap();
        lock.as_ref()
            .map(|f| f.get_aliases_text())
            .unwrap_or_default()
    }

    /// Inject additional provider models discovered at runtime (e.g., Ollama).
    /// These are checked alongside the static yaml-based models during selection.
    pub fn inject_provider_models(&self, provider: &str, models: Vec<ModelInfo>) {
        let mut lock = self.runtime_models.write().unwrap();
        lock.insert(provider.to_string(), models);
    }

    /// Rebuild the fuzzy matcher (e.g., after provider reload).
    pub fn rebuild_fuzzy(&self, user_aliases: &HashMap<String, String>) {
        // Merge static yaml models with runtime-discovered models for fuzzy matching
        let mut all_models = self.config.provider_models.clone();
        let runtime = self.runtime_models.read().unwrap();
        for (k, v) in runtime.iter() {
            all_models
                .entry(k.clone())
                .or_default()
                .extend(v.iter().cloned());
        }
        let new_fuzzy =
            FuzzyMatcher::new(&chat_models(&all_models), user_aliases, &self.config.provider_credentials);
        let mut lock = self.fuzzy.write().unwrap();
        *lock = Some(new_fuzzy);
    }

    /// Get model info by "provider/model" ID.
    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let (provider_id, model_name) = parse_model_id(model_id);
        let runtime = self.runtime_models.read().unwrap();
        runtime
            .get(provider_id)
            .and_then(|models| models.iter().find(|m| m.id == model_name))
            .or_else(|| self.config.provider_models.get(provider_id)?.iter().find(|m| m.id == model_name))
            .cloned()
    }

    /// The context window of `model_id` ("provider/model"): what the provider
    /// reported for it (Janus's `/v1/models` `context_window`, synced into the
    /// runtime models) or the catalog's number for a direct provider, else
    /// [`DEFAULT_CONTEXT_WINDOW`].
    pub fn context_window(&self, model_id: &str) -> usize {
        self.get_model_info(model_id)
            .and_then(|m| usize::try_from(m.context_window).ok())
            .filter(|&w| w > 0)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW)
    }

    /// Whether `model_id` ("provider/model") thinks: its catalog or synced
    /// capabilities list "thinking".
    pub fn thinks(&self, model_id: &str) -> bool {
        self.get_model_info(model_id)
            .is_some_and(|m| m.capabilities.iter().any(|c| c == "thinking"))
    }

    /// The model a turn is sent to, resolved once at the turn's start:
    /// `chosen` (the owner's pick, the job's or a helper's speed; a fuzzy
    /// name resolves) when it is a chat model this bot can send to, else,
    /// when nothing was chosen, the configured default (Settings → Routing
    /// → General, which ships as [`DEFAULT_CHAT_MODEL`]). Any discrepancy
    /// sends [`DEFAULT_CHAT_MODEL`].
    ///
    /// A linked agent's id is not a model choice: it is the employee a
    /// linked bot runs, sent as it is, and never replaced by the default —
    /// answered by anything else, the employee would be impersonated. When
    /// its provider can't take it, the turn fails plainly.
    pub fn resolve(&self, chosen: &str) -> String {
        let chosen = chosen.trim();
        if ai::LinkedProvider::target(chosen).is_some() {
            return chosen.to_string();
        }
        if !chosen.is_empty() {
            if self.sendable(chosen) {
                return chosen.to_string();
            }
            let id = self.resolve_fuzzy(chosen).unwrap_or_default();
            if self.sendable(&id) {
                return id;
            }
            warn!(chosen, resolved = %id, "the chosen model is not a chat model this bot can send to; sending {DEFAULT_CHAT_MODEL}");
            return DEFAULT_CHAT_MODEL.to_string();
        }
        let configured = self
            .config
            .task_routing
            .get("general")
            .filter(|m| !m.is_empty())
            .unwrap_or(&self.config.default_model);
        if self.sendable(configured) {
            return configured.clone();
        }
        DEFAULT_CHAT_MODEL.to_string()
    }

    /// The model background work runs on (chat titles and the other chores
    /// that don't fork a turn's request): the configured aux route when it
    /// is a chat model this bot can send to, else [`DEFAULT_CHAT_MODEL`].
    /// Never the cheapest row: the embedding models are the cheapest rows.
    pub fn background_model(&self) -> String {
        match self.config.task_routing.get("aux").filter(|m| self.sendable(m)) {
            Some(aux) => aux.clone(),
            None => DEFAULT_CHAT_MODEL.to_string(),
        }
    }

    /// A model a turn may be sent to: its provider is loaded (when the
    /// loaded set is known) and it is a known chat model, or a model of a
    /// loaded CLI provider.
    fn sendable(&self, model_id: &str) -> bool {
        let (provider_id, model_name) = parse_model_id(model_id);
        if provider_id.is_empty() || model_name.is_empty() {
            return false;
        }
        let loaded = self.loaded_providers.read().unwrap();
        if !loaded.is_empty() && !loaded.iter().any(|p| p == provider_id) {
            return false;
        }
        if CLI_PROVIDERS.contains(&provider_id) {
            return !loaded.is_empty();
        }
        self.get_model_info(model_id).is_some_and(|m| m.chats())
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

    fn info(id: &str, context_window: i32) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            display_name: id.into(),
            context_window,
            input_price: 0.0,
            output_price: 0.0,
            cached_input_price: 0.0,
            capabilities: vec![],
            kind: vec![],
            preferred: false,
            active: true,
        }
    }

    /// A Janus speed's window is the one Janus reported at the model sync,
    /// injected at runtime: nothing in the catalog knows it.
    #[test]
    fn a_synced_janus_window_is_the_models_window() {
        let mut config = ModelRoutingConfig::default();
        config.provider_models.insert("janus".into(), vec![info("nebo-1", 0)]);
        config.provider_models.insert("anthropic".into(), vec![info("claude-opus-4-6", 1_000_000)]);
        let selector = ModelSelector::new(config);
        selector.inject_provider_models("janus", vec![info("nebo-1", 200_000), info("nebo-1-pro", 131_072)]);
        assert_eq!(selector.get_model_info("janus/nebo-1-pro").map(|m| m.context_window), Some(131_072));
        assert_eq!(selector.context_window("janus/nebo-1-pro"), 131_072);
        assert_eq!(selector.context_window("janus/nebo-1"), 200_000, "the synced row wins over the boot floor");
        assert_eq!(selector.context_window("anthropic/claude-opus-4-6"), 1_000_000, "a direct provider keeps its catalog value");
    }

    /// A model whose window nothing reported gets the 200k default.
    #[test]
    fn an_unknown_window_is_the_one_default() {
        let selector = ModelSelector::new(ModelRoutingConfig::default());
        selector.inject_provider_models("ollama", vec![info("llama", 0)]);
        assert_eq!(selector.context_window("ollama/llama"), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(selector.context_window("janus/never-heard-of-it"), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(DEFAULT_CONTEXT_WINDOW, 200_000);
    }

    /// The owner's provider_models on 2026-09-25: the gateway's chat speeds
    /// and its embedding models, all active.
    fn gateway() -> ModelSelector {
        let with = |id: &str, window: i32, caps: &[&str]| ModelInfo {
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            ..info(id, window)
        };
        let chat = ["vision", "tools", "streaming", "code", "reasoning"];
        let mut config = ModelRoutingConfig {
            task_routing: [("general".to_string(), "janus/nebo-1".to_string())].into(),
            default_model: "janus/nebo-1".into(),
            ..Default::default()
        };
        config.provider_models.insert(
            "janus".into(),
            vec![
                with("nebo-1", 200_000, &chat),
                with("nebo-embed-small", 8_191, &["embeddings"]),
                with("nebo-embed-large", 8_191, &["embeddings"]),
                with("nebo-1-flash", 200_000, &chat),
            ],
        );
        config.provider_credentials.insert("janus".into(), true);
        let selector = ModelSelector::new(config);
        selector.set_loaded_providers(vec!["janus".into()]);
        selector
    }

    /// No path returns an embedding model: not the default, not a chosen
    /// embedding model, not a name that fuzzy-matches one; and the chosen
    /// model is what is sent.
    #[test]
    fn no_path_returns_an_embedding_model() {
        let selector = gateway();
        assert_eq!(selector.resolve(""), "janus/nebo-1", "the configured default");
        assert_eq!(selector.resolve("janus/nebo-1-flash"), "janus/nebo-1-flash", "the chosen chat model is sent as chosen");
        for chosen in ["janus/nebo-embed-small", "janus/nebo-embed-large", "nebo-embed-small", "embed"] {
            assert_eq!(selector.resolve(chosen), DEFAULT_CHAT_MODEL, "{chosen} is never a chat model");
        }
        let mut config = ModelRoutingConfig {
            task_routing: [("general".to_string(), "janus/nebo-embed-small".to_string())].into(),
            ..Default::default()
        };
        config.provider_models.insert("janus".into(), vec![ModelInfo { capabilities: vec!["embeddings".into()], ..info("nebo-embed-small", 8_191) }]);
        let misconfigured = ModelSelector::new(config);
        misconfigured.set_loaded_providers(vec!["janus".into()]);
        assert_eq!(misconfigured.resolve(""), DEFAULT_CHAT_MODEL, "an embedding model configured as the chat model");
        assert_eq!(misconfigured.background_model(), DEFAULT_CHAT_MODEL, "background work never runs on an embedding model");
    }

    /// The speeds a helper or an employee may name are chat models only.
    #[test]
    fn named_speeds_are_chat_models_only() {
        let selector = gateway();
        selector.rebuild_fuzzy(&HashMap::new());
        assert!(!selector.get_aliases_text().contains("embed"), "{}", selector.get_aliases_text());
        assert_ne!(selector.resolve_fuzzy("nebo-embed-small").as_deref(), Some("janus/nebo-embed-small"));
    }

    /// Any discrepancy sends the gateway's default chat model: a name nobody
    /// knows, a model whose provider isn't loaded.
    #[test]
    fn a_discrepancy_sends_the_default_chat_model() {
        let selector = gateway();
        assert_eq!(selector.resolve("janus/never-heard-of-it"), DEFAULT_CHAT_MODEL);
        assert_eq!(selector.resolve("gibberish"), DEFAULT_CHAT_MODEL);
        assert_eq!(selector.resolve("anthropic/claude-sonnet-4-5"), DEFAULT_CHAT_MODEL, "a provider this bot hasn't loaded");
        assert_eq!(DEFAULT_CHAT_MODEL, "janus/nebo-1");
    }

    /// A linked employee's id is sent as it is: never fuzzy-matched onto a
    /// model, never replaced by the default. Every other chosen name that
    /// isn't a chat model this bot can send to still sends the default.
    #[test]
    fn a_linked_agent_is_sent_as_itself() {
        let selector = gateway();
        selector.set_loaded_providers(vec!["janus".into(), "linked".into()]);
        selector.rebuild_fuzzy(&HashMap::new());
        let hermes = ai::LinkedProvider::model_id("a736730b-86e3-4a70-9a44-5e51724acf6e", "hermes");
        assert_eq!(selector.resolve(&hermes), hermes);
        assert_eq!(selector.resolve_fuzzy(&hermes).as_deref(), Some(hermes.as_str()));
        assert_eq!(selector.resolve_fuzzy("linked/nebo-1"), None, "a malformed linked id names nothing");
        assert_eq!(selector.resolve("linked/nebo-1"), DEFAULT_CHAT_MODEL);
        assert_eq!(selector.resolve("nebo-2-ultra-bogus"), DEFAULT_CHAT_MODEL, "a bogus preference");
    }

    /// Background work runs on the configured aux route when it is a chat
    /// model, else the default chat model; never the cheapest row (the
    /// embedding models are priced lowest).
    #[test]
    fn background_work_runs_on_a_chat_model() {
        let selector = gateway();
        assert_eq!(selector.background_model(), DEFAULT_CHAT_MODEL);
        let mut config = ModelRoutingConfig {
            task_routing: [("aux".to_string(), "janus/nebo-1-flash".to_string())].into(),
            ..Default::default()
        };
        config.provider_models.insert("janus".into(), vec![ModelInfo { capabilities: vec!["tools".into()], ..info("nebo-1-flash", 200_000) }]);
        assert_eq!(ModelSelector::new(config).background_model(), "janus/nebo-1-flash");
    }

    /// A model's real window is used, however small: a 32k local model
    /// checkpoints at 32k.
    #[test]
    fn a_small_real_window_is_the_models_window() {
        let selector = gateway();
        selector.inject_provider_models("ollama", vec![info("small", 32_768)]);
        assert_eq!(selector.context_window("ollama/small"), 32_768);
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
                escalation: String::new(),
                primary: "anthropic/claude-sonnet-4-20250514".into(),
                fallbacks: vec![],
            }),
            task_routing: None,
            lane_routing: None,
            aliases: vec![],
            providers,
            cli_providers: vec![],
        };

        let routing = ModelRoutingConfig::from_models_config(
            &models_cfg,
            &["anthropic".into()],
            &HashMap::new(),
        );
        assert_eq!(routing.default_model, "anthropic/claude-sonnet-4-20250514");
        assert!(
            routing
                .provider_credentials
                .get("anthropic")
                .copied()
                .unwrap_or(false)
        );
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
                cached_input_price: 0.0,
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
                cached_input_price: 0.0,
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
            default_model: "anthropic/claude-sonnet-4".into(),
            provider_models,
            provider_credentials: creds,
        };

        let selector = ModelSelector::new(config);
        // Only load anthropic — openai models should be filtered out
        selector.set_loaded_providers(vec!["anthropic".into()]);

        let selected = selector.resolve("");
        // Should pick an anthropic model since openai is not loaded
        assert!(
            selected.contains("anthropic"),
            "Expected anthropic model, got: {}",
            selected
        );
    }

    /// Nothing configured this bot can send to: the default chat model,
    /// whatever else is loaded (no roaming to whichever provider is first).
    #[test]
    fn nothing_sendable_configured_sends_the_default_chat_model() {
        let mut config = ModelRoutingConfig { default_model: "anthropic/claude-sonnet-4-5".into(), ..Default::default() };
        config.provider_models.insert("anthropic".into(), vec![info("claude-sonnet-4-5", 200_000)]);
        let selector = ModelSelector::new(config);
        selector.set_loaded_providers(vec!["claude-code".into(), "janus".into()]);
        assert_eq!(selector.resolve(""), DEFAULT_CHAT_MODEL);
    }
}
