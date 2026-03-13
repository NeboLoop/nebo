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

/// Task-based model routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRouting {
    #[serde(default)]
    pub vision: String,
    #[serde(default)]
    pub audio: String,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub general: String,
    #[serde(default)]
    pub fallbacks: HashMap<String, Vec<String>>,
}

/// Lane-specific model routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneRouting {
    #[serde(default)]
    pub heartbeat: String,
    #[serde(default)]
    pub events: String,
    #[serde(default)]
    pub comm: String,
    #[serde(default)]
    pub subagent: String,
}

/// Default model selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
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
    pub lane_routing: Option<LaneRouting>,
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
    pub fn update_model(&mut self, provider: &str, model_id: &str, update: ModelUpdate) -> Result<(), String> {
        let models = self.providers.get_mut(provider).ok_or("provider not found")?;
        let model = models.iter_mut().find(|m| m.id == model_id).ok_or("model not found")?;
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
        let cp = self.cli_providers.iter_mut().find(|c| c.id == cli_id).ok_or("CLI provider not found")?;
        cp.active = Some(active);
        self.save()
    }

    /// Get the default model ID for a given provider name.
    /// Checks defaults.primary first, then fallbacks, then first model in provider list.
    pub fn default_model_for_provider(&self, provider: &str) -> Option<String> {
        // Check if defaults.primary targets this provider
        if let Some(ref defaults) = self.defaults {
            if let Some(model_id) = Self::extract_model_for_provider(&defaults.primary, provider) {
                return Some(model_id);
            }
            for fb in &defaults.fallbacks {
                if let Some(model_id) = Self::extract_model_for_provider(fb, provider) {
                    return Some(model_id);
                }
            }
        }
        // Fall back to first model in provider's model list
        self.providers
            .get(provider)
            .and_then(|models| models.first())
            .map(|m| m.id.clone())
    }

    /// Get the model ID for a specific task type (vision, reasoning, code, general, audio).
    /// Returns just the model ID portion (without the "provider/" prefix).
    pub fn model_for_task(&self, task: &str) -> Option<String> {
        self.task_routing.as_ref().and_then(|tr| {
            let full = match task {
                "vision" => &tr.vision,
                "audio" => &tr.audio,
                "reasoning" => &tr.reasoning,
                "code" => &tr.code,
                "general" => &tr.general,
                _ => return None,
            };
            if full.is_empty() {
                return None;
            }
            Some(full.split('/').last().unwrap_or(full).to_string())
        })
    }

    /// Get the cheapest/fallback model ID (for sidecar tasks).
    /// Returns the first default fallback model ID.
    pub fn sidecar_model(&self) -> Option<String> {
        self.defaults.as_ref().and_then(|d| {
            d.fallbacks.first().map(|spec| {
                spec.split('/').last().unwrap_or(spec).to_string()
            })
        })
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

    /// Load models config: try data_dir/models.yaml first, fall back to embedded.
    pub fn load() -> Self {
        // Try user's data directory first
        if let Ok(dir) = data_dir() {
            let path = dir.join(types::constants::files::MODELS_YAML);
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(mut cfg) = serde_yaml::from_str::<ModelsConfig>(&data) {
                    // Merge missing sections from embedded defaults
                    if let Ok(defaults) = serde_yaml::from_str::<ModelsConfig>(EMBEDDED_MODELS_YAML) {
                        if cfg.cli_providers.is_empty() && !defaults.cli_providers.is_empty() {
                            cfg.cli_providers = defaults.cli_providers;
                        }
                        if !cfg.providers.contains_key("janus") {
                            if let Some(janus_models) = defaults.providers.get("janus") {
                                cfg.providers.insert("janus".into(), janus_models.clone());
                            }
                        }
                    }

                    // Migrate janus model IDs: strip legacy prefixes
                    if let Some(janus_models) = cfg.providers.get_mut("janus") {
                        for model in janus_models.iter_mut() {
                            if let Some(stripped) = model.id.strip_prefix("janus/") {
                                model.id = stripped.to_string();
                            }
                            if let Some(stripped) = model.id.strip_prefix("neboloop/") {
                                model.id = stripped.to_string();
                            }
                            // Migrate old "janus" model ID to "nebo-1"
                            if model.id == "janus" {
                                model.id = "nebo-1".to_string();
                                if model.display_name == "Janus" {
                                    model.display_name = "Nebo 1".to_string();
                                }
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
            lane_routing: None,
            aliases: Vec::new(),
            providers: HashMap::new(),
            cli_providers: Vec::new(),
        })
    }
}
