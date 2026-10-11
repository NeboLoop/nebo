use std::collections::HashMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::data_dir;

const EMBEDDED_MODELS_YAML: &str = include_str!("models.yaml");

/// Pricing per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default, rename = "cachedInput")]
    pub cached_input: f64,
}

/// A single model definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(default, rename = "contextWindow")]
    pub context_window: i64,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub kind: Vec<String>,
    #[serde(default)]
    pub preferred: bool,
    #[serde(default)]
    pub active: Option<bool>,
}

impl ModelDef {
    /// Whether the model is active (defaults to true if not specified).
    pub fn is_active(&self) -> bool {
        self.active.unwrap_or(true)
    }
}

/// The bot's routes in models.yaml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskRouting {
    #[serde(default)]
    pub vision: String,
    /// The bot's default: a model, or an intelligence pack (`pack/<id>`).
    #[serde(default)]
    pub general: String,
    /// Cheap-model route for internal side-tasks (titles, memory extraction,
    /// tool summaries, auto-continue judge). "provider/model" format; empty = unset
    /// (each side-task keeps its default provider selection).
    #[serde(default)]
    pub aux: String,
}

/// Default model selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub primary: String,
}

/// User-friendly model alias.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAlias {
    #[serde(default)]
    pub alias: String,
    #[serde(default, rename = "modelId")]
    pub model_id: String,
}

/// CLI provider definition from models.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProviderDef {
    pub id: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default, rename = "installHint")]
    pub install_hint: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, rename = "defaultModel")]
    pub default_model: String,
    #[serde(default)]
    pub active: Option<bool>,
}

impl CliProviderDef {
    /// Whether the CLI provider is active (defaults to false if not specified).
    pub fn is_active(&self) -> bool {
        self.active.unwrap_or(false)
    }
}

/// Top-level models.yaml config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub defaults: Option<Defaults>,
    #[serde(default)]
    pub task_routing: Option<TaskRouting>,
    #[serde(default)]
    pub aliases: Vec<ModelAlias>,
    #[serde(default)]
    pub providers: HashMap<String, Vec<ModelDef>>,
    #[serde(default)]
    pub cli_providers: Vec<CliProviderDef>,
}

/// Update for a single model's mutable fields.
pub struct ModelUpdate {
    pub active: Option<bool>,
    pub kind: Option<Vec<String>>,
    pub preferred: Option<bool>,
}

impl ModelsConfig {
    /// Save config to data_dir/models.yaml.
    pub fn save(&self) -> Result<(), String> {
        let dir = data_dir().map_err(|e| e.to_string())?;
        let path = dir.join(types::constants::files::MODELS_YAML);
        fs::create_dir_all(&dir).map_err(|e| format!("failed to create data dir: {e}"))?;
        let data = serde_yaml::to_string(self).map_err(|e| format!("failed to serialize: {e}"))?;
        fs::write(&path, data).map_err(|e| format!("failed to write models.yaml: {e}"))?;
        Ok(())
    }

    /// Update a model's settings (active, kind, preferred) and save.
    pub fn update_model(
        &mut self,
        provider: &str,
        model_id: &str,
        update: ModelUpdate,
    ) -> Result<(), String> {
        let models = self
            .providers
            .get_mut(provider)
            .ok_or("provider not found")?;
        let model = models
            .iter_mut()
            .find(|m| m.id == model_id)
            .ok_or("model not found")?;
        if let Some(active) = update.active {
            model.active = Some(active);
        }
        if let Some(kind) = update.kind {
            model.kind = kind;
        }
        if let Some(preferred) = update.preferred {
            model.preferred = preferred;
        }
        self.save()
    }

    /// Set a CLI provider's active state and save.
    pub fn set_cli_provider_active(&mut self, cli_id: &str, active: bool) -> Result<(), String> {
        let cp = self
            .cli_providers
            .iter_mut()
            .find(|c| c.id == cli_id)
            .ok_or("CLI provider not found")?;
        cp.active = Some(active);
        self.save()
    }

    /// The model a provider's connection runs when the connection names
    /// none: `defaults.primary` when it names this provider, else the
    /// provider's first catalog row.
    pub fn default_model_for_provider(&self, provider: &str) -> Option<String> {
        if let Some(model_id) =
            self.defaults.as_ref().and_then(|d| Self::extract_model_for_provider(&d.primary, provider))
        {
            return Some(model_id);
        }
        self.providers
            .get(provider)
            .and_then(|models| models.first())
            .map(|m| m.id.clone())
    }

    /// Resolve the model a sidecar task should run on for the provider that is
    /// about to be called.
    ///
    /// The model ID must belong to `provider` — a bare ID lifted from another
    /// provider's spec (handing Anthropic's `claude-sonnet-4-6` to the Janus
    /// gateway, say) names a model that provider has never heard of, and the
    /// request fails in a way that reads like the sidecar simply had nothing
    /// to say. `None` means "let the provider pick its own default", which is
    /// always a model that provider can actually serve.
    pub fn sidecar_model(&self, provider: &str) -> Option<String> {
        // Prefer the explicit vision route when it targets this provider.
        if let Some(ref tr) = self.task_routing {
            if let Some(model) = Self::extract_model_for_provider(&tr.vision, provider) {
                return Some(model);
            }
        }
        // Otherwise the provider's own default, which is provider-correct by
        // construction.
        self.default_model_for_provider(provider)
    }

    /// Extract model ID from "provider/model" format if it matches the given provider.
    fn extract_model_for_provider(spec: &str, provider: &str) -> Option<String> {
        spec.split_once('/').and_then(|(p, model)| {
            if p == provider {
                Some(model.to_string())
            } else {
                None
            }
        })
    }

    /// The owner's file, rewritten once to what Nebo reads now (the server's
    /// `migration::migrate_models_yaml`): a Nebo AI speed chosen as the
    /// bot's default (`defaults.primary`, `task_routing.general`) becomes the Nebo AI pack at that level, and the
    /// Janus catalog's ids lose the `janus/` and `neboai/` prefixes and the
    /// old `janus` id. True when anything changed.
    pub fn upgrade(&mut self) -> bool {
        const SPEED_PACKS: [(&str, &str); 4] = [
            ("nebo-1", "pack/nebo-ai"),
            ("nebo-1-flash", "pack/nebo-ai/instant"),
            ("nebo-1-medium", "pack/nebo-ai/medium"),
            ("nebo-1-pro", "pack/nebo-ai/high"),
        ];
        let mut changed = false;
        let mut to_pack = |value: &mut String| {
            let speed = value.trim();
            let speed = speed.strip_prefix("janus/").unwrap_or(speed);
            if let Some((_, pack)) = SPEED_PACKS.iter().find(|(s, _)| *s == speed) {
                *value = pack.to_string();
                changed = true;
            }
        };
        if let Some(defaults) = self.defaults.as_mut() {
            to_pack(&mut defaults.primary);
        }
        if let Some(tr) = self.task_routing.as_mut() {
            to_pack(&mut tr.general);
        }
        for model in self.providers.get_mut("janus").into_iter().flatten() {
            let id = model.id.trim_start_matches("janus/").trim_start_matches("neboai/");
            let id = if id == "janus" { "nebo-1" } else { id };
            if id != model.id {
                if model.id == "janus" && model.display_name == "Janus" {
                    model.display_name = "Nebo 1".to_string();
                }
                model.id = id.to_string();
                changed = true;
            }
        }
        changed
    }

    /// The owner's own models.yaml as it is on disk, if there is one that
    /// reads.
    pub fn load_owned() -> Option<Self> {
        let path = data_dir().ok()?.join(types::constants::files::MODELS_YAML);
        serde_yaml::from_str(&fs::read_to_string(path).ok()?).ok()
    }

    /// Load models config: try data_dir/models.yaml first, fall back to embedded.
    pub fn load() -> Self {
        // Try user's data directory first
        if let Ok(dir) = data_dir() {
            let path = dir.join(types::constants::files::MODELS_YAML);
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(mut cfg) = serde_yaml::from_str::<ModelsConfig>(&data) {
                    // Merge missing sections from embedded defaults
                    if let Ok(defaults) = serde_yaml::from_str::<ModelsConfig>(EMBEDDED_MODELS_YAML)
                    {
                        if cfg.cli_providers.is_empty() && !defaults.cli_providers.is_empty() {
                            cfg.cli_providers = defaults.cli_providers;
                        }
                        if !cfg.providers.contains_key("janus") {
                            if let Some(janus_models) = defaults.providers.get("janus") {
                                cfg.providers.insert("janus".into(), janus_models.clone());
                            }
                        }
                    }

                    return cfg;
                }
            }
        }

        // Fall back to embedded
        serde_yaml::from_str(EMBEDDED_MODELS_YAML).unwrap_or_else(|_| ModelsConfig {
            version: "1.0".into(),
            defaults: None,
            task_routing: None,
            aliases: Vec::new(),
            providers: HashMap::new(),
            cli_providers: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ModelsConfig {
        serde_yaml::from_str(EMBEDDED_MODELS_YAML).expect("embedded models.yaml must parse")
    }

    // The sidecar used to take defaults.fallbacks[0] ("anthropic/claude-sonnet-4-6"),
    // strip the provider, and hand the bare ID to whichever provider was first —
    // usually the Janus gateway, which has never heard of it. The request failed
    // in a way that looked like the sidecar just had nothing to say.
    #[test]
    fn sidecar_model_never_leaks_across_providers() {
        let cfg = config();
        for provider in ["janus", "anthropic", "openai", "google"] {
            if let Some(model) = cfg.sidecar_model(provider) {
                assert!(
                    !model.contains('/'),
                    "{provider}: model ID must not keep a provider prefix, got {model}"
                );
                let owned_by_provider = cfg
                    .providers
                    .get(provider)
                    .map(|models| models.iter().any(|m| m.id == model))
                    .unwrap_or(false);
                let from_vision_route = cfg
                    .task_routing
                    .as_ref()
                    .and_then(|tr| ModelsConfig::extract_model_for_provider(&tr.vision, provider))
                    .is_some_and(|m| m == model);
                assert!(
                    owned_by_provider || from_vision_route,
                    "{provider}: resolved {model}, which that provider does not serve"
                );
            }
        }
    }

    #[test]
    fn sidecar_model_prefers_the_vision_route() {
        let cfg = config();
        // models.yaml routes vision at janus/nebo-1.
        assert_eq!(cfg.sidecar_model("janus").as_deref(), Some("nebo-1"));
    }

    #[test]
    fn sidecar_model_is_none_for_unknown_provider() {
        assert!(config().sidecar_model("not-a-provider").is_none());
    }

    /// The one-time rewrite of the owner's file: a speed chosen as the
    /// bot's default becomes the Nebo AI pack at its level; a pack or any
    /// other model stays; the Janus catalog's old ids are renamed; a file
    /// that needs nothing reports no change.
    #[test]
    fn upgrade_names_the_nebo_ai_pack_where_a_speed_was_chosen() {
        let mut cfg: ModelsConfig = serde_yaml::from_str(
            "defaults:\n  primary: janus/nebo-1\ntask_routing:\n  general: nebo-1-pro\n  aux: janus/nebo-1-flash\nproviders:\n  janus:\n    - id: janus\n      displayName: Janus\n    - id: neboai/nebo-embed-small\n",
        )
        .unwrap();
        assert!(cfg.upgrade());
        assert_eq!(cfg.defaults.as_ref().unwrap().primary, "pack/nebo-ai");
        let tr = cfg.task_routing.as_ref().unwrap();
        assert_eq!(tr.general, "pack/nebo-ai/high");
        assert_eq!(tr.aux, "janus/nebo-1-flash", "background work is not a choice of the bot's default");
        let ids: Vec<(&str, &str)> =
            cfg.providers["janus"].iter().map(|m| (m.id.as_str(), m.display_name.as_str())).collect();
        assert_eq!(ids, [("nebo-1", "Nebo 1"), ("nebo-embed-small", "")]);
        assert!(!cfg.upgrade(), "nothing left to change");

        let mut mine: ModelsConfig =
            serde_yaml::from_str("defaults:\n  primary: pack/abc/low\ntask_routing:\n  general: openai/gpt-5.4\n").unwrap();
        assert!(!mine.upgrade());
        assert_eq!(mine.task_routing.unwrap().general, "openai/gpt-5.4");
    }

    #[test]
    fn the_bot_ships_on_the_nebo_ai_pack() {
        let cfg = config();
        assert_eq!(cfg.defaults.unwrap().primary, "pack/nebo-ai");
        assert_eq!(cfg.task_routing.unwrap().general, "pack/nebo-ai");
    }
}
