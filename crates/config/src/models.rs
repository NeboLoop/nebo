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
    /// The bot's default: a model, or an intelligence pack (`pack/<id>`).
    #[serde(default)]
    pub general: String,
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

    /// The one-time rewrite of the owner's file: a speed chosen as the
    /// bot's default becomes the Nebo AI pack at its level; a pack or any
    /// other model stays; the Janus catalog's old ids are renamed; a file
    /// that needs nothing reports no change.
    #[test]
    fn upgrade_names_the_nebo_ai_pack_where_a_speed_was_chosen() {
        let mut cfg: ModelsConfig = serde_yaml::from_str(
            "defaults:\n  primary: janus/nebo-1\ntask_routing:\n  general: nebo-1-pro\nproviders:\n  janus:\n    - id: janus\n      displayName: Janus\n    - id: neboai/nebo-embed-small\n",
        )
        .unwrap();
        assert!(cfg.upgrade());
        assert_eq!(cfg.defaults.as_ref().unwrap().primary, "pack/nebo-ai");
        let tr = cfg.task_routing.as_ref().unwrap();
        assert_eq!(tr.general, "pack/nebo-ai/high");
        let ids: Vec<(&str, &str)> =
            cfg.providers["janus"].iter().map(|m| (m.id.as_str(), m.display_name.as_str())).collect();
        assert_eq!(ids, [("nebo-1", "Nebo 1"), ("nebo-embed-small", "")]);
        assert!(!cfg.upgrade(), "nothing left to change");

        let mut mine: ModelsConfig =
            serde_yaml::from_str("defaults:\n  primary: pack/abc/low\ntask_routing:\n  general: openai/gpt-5.4\n").unwrap();
        assert!(!mine.upgrade());
        assert_eq!(mine.task_routing.unwrap().general, "openai/gpt-5.4");
    }

    /// Nebo names no model for a provider the owner adds: the catalog holds
    /// only Nebo AI's boot floor.
    #[test]
    fn the_catalog_names_no_model_for_an_owners_provider() {
        let cfg = config();
        let providers: Vec<&str> = cfg.providers.keys().map(String::as_str).collect();
        assert_eq!(providers, ["janus"]);
    }

    #[test]
    fn the_bot_ships_on_the_nebo_ai_pack() {
        let cfg = config();
        assert_eq!(cfg.defaults.unwrap().primary, "pack/nebo-ai");
        assert_eq!(cfg.task_routing.unwrap().general, "pack/nebo-ai");
    }
}
