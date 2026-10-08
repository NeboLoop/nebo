//! A provider as a connection: one key the owner added, and the models it
//! offers — the vendor's catalog, models typed in by hand, and models picked
//! while browsing the provider's own list. Nothing is limited to Nebo's
//! catalog (owner, 2026-10-07). A model is a chat model or a decision model
//! (Jev and other SystemOne-compatible ones). Design: neboloop
//! `docs/prd/intelligence-packs.md` §1a.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::response::Json;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// The abilities a model row shows.
const ABILITIES: &[&str] = &["vision", "tools", "thinking"];
/// How long a provider's fetched list is kept before it is fetched again.
const CATALOG_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// One of a connection's models, as the app shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionModel {
    pub model_id: String,
    pub display_name: String,
    /// `chat` or `decision`.
    pub kind: String,
    pub context_window: Option<i64>,
    pub capabilities: Vec<String>,
    /// `catalog`, `added` or `browsed`.
    pub source: String,
    pub is_active: bool,
}

/// A connection's key in `provider_models` and in model strings.
pub fn connection_key(profile: &db::models::AuthProfile) -> String {
    ai::connection_id(&profile.provider, &profile.id)
}

/// The abilities among `caps` a model row shows (`reasoning` reads as
/// `thinking`), in a fixed order.
fn abilities(caps: &[String]) -> Vec<String> {
    let has = |a: &str| caps.iter().any(|c| c == a || (a == "thinking" && c == "reasoning"));
    ABILITIES.iter().filter(|a| has(a)).map(|a| a.to_string()).collect()
}

fn model_of_row(row: &db::models::ProviderModel) -> ConnectionModel {
    let caps: Vec<String> = row.capabilities.as_deref().and_then(|c| serde_json::from_str(c).ok()).unwrap_or_default();
    ConnectionModel {
        model_id: row.model_id.clone(),
        display_name: row.display_name.clone(),
        kind: row.model_kind.clone(),
        context_window: row.context_window,
        capabilities: abilities(&caps),
        source: row.source.clone(),
        is_active: row.is_active.unwrap_or(0) == 1,
    }
}

/// Every model a connection offers: its own rows (typed, browsed, or a
/// catalog model it toggled) first, then the vendor's catalog models it has
/// no row for.
pub fn connection_models(
    store: &db::Store,
    catalog: &config::ModelsConfig,
    profile: &db::models::AuthProfile,
) -> Vec<ConnectionModel> {
    let own: Vec<ConnectionModel> = store
        .list_provider_models(&connection_key(profile))
        .unwrap_or_default()
        .iter()
        .map(model_of_row)
        .collect();
    let mut out = own.clone();
    for m in catalog.providers.get(&profile.provider).into_iter().flatten() {
        if own.iter().any(|o| o.model_id == m.id) {
            continue;
        }
        out.push(ConnectionModel {
            model_id: m.id.clone(),
            display_name: if m.display_name.is_empty() { m.id.clone() } else { m.display_name.clone() },
            kind: "chat".into(),
            context_window: (m.context_window > 0).then_some(m.context_window),
            capabilities: abilities(&m.capabilities),
            source: "catalog".into(),
            is_active: m.is_active(),
        });
    }
    out
}

fn profile(state: &AppState, id: &str) -> Result<db::models::AuthProfile, types::NeboError> {
    state.store.get_auth_profile(id)?.ok_or(types::NeboError::NotFound)
}

/// GET /api/v1/providers/{id}/models — every model this connection offers.
pub async fn list_connection_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    let models = connection_models(&state.store, &config::ModelsConfig::load(), &p);
    Ok(Json(serde_json::json!({ "models": models })))
}

/// What the app sends to add or change one of a connection's models.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelBody {
    pub model_id: String,
    pub kind: Option<String>,
    pub display_name: Option<String>,
    pub context_window: Option<i64>,
    pub capabilities: Option<Vec<String>>,
    pub is_active: Option<bool>,
    /// `browsed` when picked from the provider's list; else typed by hand.
    pub source: Option<String>,
}

fn kind_of(kind: Option<&str>) -> Result<String, types::NeboError> {
    match kind.unwrap_or("chat") {
        k @ ("chat" | "decision") => Ok(k.to_string()),
        other => Err(types::NeboError::Validation(format!("{other} isn't a model kind (chat or decision)."))),
    }
}

/// Save one model of a connection and let the bot send to it at once.
fn put(state: &AppState, key: &str, m: &ConnectionModel) -> Result<(), types::NeboError> {
    let caps = serde_json::to_string(&m.capabilities).unwrap_or_else(|_| "[]".into());
    state.store.put_connection_model(
        key,
        &m.model_id,
        &m.display_name,
        &m.kind,
        m.context_window,
        &caps,
        &m.source,
        m.is_active,
    )?;
    crate::inject_connection_models(&state.store, state.harness.selector());
    Ok(())
}

/// POST /api/v1/providers/{id}/models — add a model by its id (typed by
/// hand), or one picked while browsing (`source: "browsed"`).
pub async fn add_connection_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ModelBody>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    let model_id = body.model_id.trim().to_string();
    if model_id.is_empty() || model_id.contains(char::is_whitespace) {
        return Err(to_error_response(types::NeboError::Validation("A model needs its id, like claude-sonnet-5.".into())));
    }
    let model = ConnectionModel {
        display_name: body.display_name.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(&model_id).to_string(),
        kind: kind_of(body.kind.as_deref()).map_err(to_error_response)?,
        context_window: body.context_window.filter(|w| *w > 0),
        capabilities: abilities(&body.capabilities.unwrap_or_default()),
        source: if body.source.as_deref() == Some("browsed") { "browsed".into() } else { "added".into() },
        is_active: body.is_active.unwrap_or(true),
        model_id,
    };
    put(&state, &connection_key(&p), &model).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "model": model })))
}

/// PUT /api/v1/providers/{id}/models/{modelId} — turn a model on or off, or
/// change its kind, context or abilities (a catalog model gets its own row).
pub async fn update_connection_model(
    State(state): State<AppState>,
    Path((id, model_id)): Path<(String, String)>,
    Json(body): Json<ModelBody>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    let mut model = connection_models(&state.store, &config::ModelsConfig::load(), &p)
        .into_iter()
        .find(|m| m.model_id == model_id)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    if let Some(active) = body.is_active {
        model.is_active = active;
    }
    if body.kind.is_some() {
        model.kind = kind_of(body.kind.as_deref()).map_err(to_error_response)?;
    }
    if let Some(w) = body.context_window {
        model.context_window = (w > 0).then_some(w);
    }
    if let Some(caps) = body.capabilities {
        model.capabilities = abilities(&caps);
    }
    if let Some(name) = body.display_name.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        model.display_name = name.to_string();
    }
    put(&state, &connection_key(&p), &model).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "model": model })))
}

/// DELETE /api/v1/providers/{id}/models/{modelId} — remove a model the
/// connection added (a catalog model falls back to the catalog's default).
pub async fn delete_connection_model(
    State(state): State<AppState>,
    Path((id, model_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    if !state.store.delete_connection_model(&connection_key(&p), &model_id).map_err(to_error_response)? {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    crate::inject_connection_models(&state.store, state.harness.selector());
    Ok(Json(serde_json::json!({ "deleted": model_id })))
}

// ── Browsing a provider's own list ─────────────────────────────────

/// One model in a provider's own list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub model_id: String,
    pub display_name: String,
    /// The model's family ("Claude Sonnet"), for grouping and Latest.
    pub family: String,
    pub kind: String,
    pub context_window: Option<i64>,
    pub capabilities: Vec<String>,
    /// $ per 1M tokens, when the provider says.
    pub pricing: Option<Pricing>,
    pub created: Option<i64>,
    pub added: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
}

/// A model's family from its display name: the vendor prefix dropped
/// ("Anthropic: …") and every word holding a digit ("4.5", "GPT-5") left out.
/// ponytail: a naming heuristic; names that break it group by their full name.
pub fn family_of(name: &str) -> String {
    let name = name.rsplit_once(": ").map(|(_, n)| n).unwrap_or(name);
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| !w.chars().any(|c| c.is_ascii_digit()) && !w.starts_with('('))
        .collect();
    if words.is_empty() { name.trim().to_string() } else { words.join(" ") }
}

/// OpenRouter's `/api/v1/models`: abilities, context and prices per model.
pub fn parse_openrouter(body: &serde_json::Value) -> Vec<CatalogModel> {
    let per_million = |v: &serde_json::Value| -> Option<f64> {
        v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()).map(|p| p * 1_000_000.0)
    };
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let name = m["name"].as_str().unwrap_or(&id).to_string();
            let has = |list: &serde_json::Value, v: &str| list.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(v)));
            let params = &m["supported_parameters"];
            let mut caps = Vec::new();
            if has(&m["architecture"]["input_modalities"], "image") {
                caps.push("vision".to_string());
            }
            if has(params, "tools") {
                caps.push("tools".to_string());
            }
            if has(params, "reasoning") || has(params, "include_reasoning") {
                caps.push("thinking".to_string());
            }
            let pricing = match (per_million(&m["pricing"]["prompt"]), per_million(&m["pricing"]["completion"])) {
                (Some(input), Some(output)) => Some(Pricing { input, output }),
                _ => None,
            };
            Some(CatalogModel {
                family: family_of(&name),
                display_name: name.rsplit_once(": ").map(|(_, n)| n.to_string()).unwrap_or(name),
                model_id: id,
                kind: "chat".into(),
                context_window: m["context_length"].as_i64(),
                capabilities: caps,
                pricing,
                created: m["created"].as_i64(),
                added: false,
            })
        })
        .collect()
}

/// An OpenAI-compatible `/v1/models`: ids only.
pub fn parse_openai_list(body: &serde_json::Value) -> Vec<CatalogModel> {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            Some(CatalogModel {
                family: family_of(&id.replace(['-', '_'], " ")),
                display_name: id.clone(),
                model_id: id,
                kind: "chat".into(),
                context_window: None,
                capabilities: Vec::new(),
                pricing: None,
                created: m["created"].as_i64(),
                added: false,
            })
        })
        .collect()
}

/// A connection's fetched list, kept for [`CATALOG_TTL`].
fn cached() -> &'static Mutex<HashMap<String, (Instant, Vec<CatalogModel>)>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, (Instant, Vec<CatalogModel>)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The provider's own list, or None when it has none worth browsing
/// (Anthropic, OpenAI and Google list ids only: their models are added by id).
async fn fetch_catalog(p: &db::models::AuthProfile) -> Option<Vec<CatalogModel>> {
    if let Some((at, list)) = cached().lock().ok()?.get(&p.id).cloned()
        && at.elapsed() < CATALOG_TTL
    {
        return Some(list);
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().ok()?;
    let list = match p.provider.as_str() {
        "openrouter" => {
            let body: serde_json::Value = client.get("https://openrouter.ai/api/v1/models").send().await.ok()?.json().await.ok()?;
            parse_openrouter(&body)
        }
        "openai_compatible" => {
            let base = p.base_url.as_deref()?.trim_end_matches('/');
            let resp = client.get(format!("{base}/models")).bearer_auth(auth::credential::profile_key(p)).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            parse_openai_list(&resp.json().await.ok()?)
        }
        _ => return None,
    };
    if let Ok(mut c) = cached().lock() {
        c.insert(p.id.clone(), (Instant::now(), list.clone()));
    }
    Some(list)
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct CatalogQuery {
    pub search: Option<String>,
    pub kind: Option<String>,
}

/// The list filtered by `search` (id or name) and `kind`, newest first, each
/// marked when the connection already has it.
pub fn filter_catalog(list: &[CatalogModel], q: &CatalogQuery, have: &[String]) -> Vec<CatalogModel> {
    let needle = q.search.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    let mut out: Vec<CatalogModel> = list
        .iter()
        .filter(|m| q.kind.as_deref().is_none_or(|k| m.kind == k))
        .filter(|m| {
            needle.as_deref().is_none_or(|n| m.model_id.to_lowercase().contains(n) || m.display_name.to_lowercase().contains(n))
        })
        .cloned()
        .map(|mut m| {
            m.added = have.contains(&m.model_id);
            m
        })
        .collect();
    out.sort_by(|a, b| b.created.unwrap_or(0).cmp(&a.created.unwrap_or(0)).then(a.model_id.cmp(&b.model_id)));
    out
}

/// GET /api/v1/providers/{id}/catalog?search=&kind= — browse the provider's
/// own list. Nothing is added until the owner picks it.
pub async fn browse_connection_catalog(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<CatalogQuery>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    let Some(list) = fetch_catalog(&p).await else {
        return Ok(Json(serde_json::json!({ "browsable": false, "total": 0, "models": [] })));
    };
    let have: Vec<String> = connection_models(&state.store, &config::ModelsConfig::load(), &p)
        .into_iter()
        .map(|m| m.model_id)
        .collect();
    let models = filter_catalog(&list, &q, &have);
    let refreshed_at = cached()
        .lock()
        .ok()
        .and_then(|c| c.get(&p.id).map(|(at, _)| chrono::Utc::now().timestamp() - at.elapsed().as_secs() as i64));
    Ok(Json(serde_json::json!({ "browsable": true, "total": list.len(), "refreshedAt": refreshed_at, "models": models })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_family_drops_the_vendor_and_the_version() {
        assert_eq!(family_of("Anthropic: Claude Sonnet 4.5"), "Claude Sonnet");
        assert_eq!(family_of("Claude Haiku 4.5 (free)"), "Claude Haiku");
        assert_eq!(family_of("llama 4 maverick"), "llama maverick");
        assert_eq!(family_of("GPT-5"), "GPT-5", "nothing left: the whole name");
    }

    #[test]
    fn openrouter_models_carry_abilities_context_and_prices() {
        let body = serde_json::json!({ "data": [
            { "id": "anthropic/claude-sonnet-5", "name": "Anthropic: Claude Sonnet 5", "created": 1790000000,
              "context_length": 1000000, "architecture": { "input_modalities": ["text", "image"] },
              "supported_parameters": ["tools", "reasoning"], "pricing": { "prompt": "0.000003", "completion": "0.000015" } },
            { "id": "meta/llama-x", "name": "Llama X", "created": 1700000000, "context_length": 131072,
              "architecture": { "input_modalities": ["text"] }, "supported_parameters": [], "pricing": {} }
        ]});
        let list = parse_openrouter(&body);
        assert_eq!(list.len(), 2);
        let s = &list[0];
        assert_eq!(s.display_name, "Claude Sonnet 5");
        assert_eq!(s.family, "Claude Sonnet");
        assert_eq!(s.capabilities, vec!["vision", "tools", "thinking"]);
        assert_eq!(s.context_window, Some(1_000_000));
        assert_eq!(s.pricing, Some(Pricing { input: 3.0, output: 15.0 }));
        assert!(list[1].capabilities.is_empty() && list[1].pricing.is_none());
    }

    #[test]
    fn browsing_filters_marks_added_and_puts_the_newest_first() {
        let list = parse_openrouter(&serde_json::json!({ "data": [
            { "id": "a/old-sonnet", "name": "Old Sonnet", "created": 1 },
            { "id": "a/new-sonnet", "name": "New Sonnet", "created": 9 },
            { "id": "b/haiku", "name": "Haiku", "created": 5 }
        ]}));
        let q = CatalogQuery { search: Some("SONNET".into()), kind: None };
        let out = filter_catalog(&list, &q, &["a/old-sonnet".to_string()]);
        assert_eq!(out.iter().map(|m| m.model_id.as_str()).collect::<Vec<_>>(), vec!["a/new-sonnet", "a/old-sonnet"]);
        assert!(!out[0].added && out[1].added);
        let decisions = filter_catalog(&list, &CatalogQuery { search: None, kind: Some("decision".into()) }, &[]);
        assert!(decisions.is_empty());
    }

    #[test]
    fn an_openai_compatible_list_is_ids_only() {
        let list = parse_openai_list(&serde_json::json!({ "data": [ { "id": "llama-4-maverick", "created": 3 } ] }));
        assert_eq!(list[0].model_id, "llama-4-maverick");
        assert!(list[0].capabilities.is_empty());
    }

    /// Every key is its own provider: two Anthropic keys are told apart,
    /// the first also answers to the bare kind, OpenRouter is built, a
    /// compatible URL without its URL and a decision endpoint are not chat
    /// providers.
    #[test]
    fn every_key_is_its_own_connection() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap());
        for (id, kind, base) in [
            ("a1", "anthropic", None),
            ("a2", "anthropic", None),
            ("or", "openrouter", None),
            ("oc", "openai_compatible", None),
            ("s1", "systemone_compatible", Some("https://api.typesafe.ai")),
        ] {
            store.create_auth_profile(id, id, kind, "k", None, base, 0, 1, None, None).unwrap();
        }
        let providers = crate::build_providers(&store, &config::Config::default(), None, None);
        let ids: Vec<&str> = providers.iter().map(|p| p.id()).collect();
        for want in ["anthropic@a1", "anthropic@a2", "anthropic", "openrouter@or", "openrouter"] {
            assert!(ids.contains(&want), "{want} missing from {ids:?}");
        }
        assert_eq!(ids.iter().filter(|i| **i == "anthropic").count(), 1, "the bare kind once");
        assert!(!ids.iter().any(|i| i.starts_with("openai_compatible") || i.starts_with("systemone")), "{ids:?}");
    }

    /// A connection's models: its own rows first, then the vendor's catalog
    /// models it has no row for; a toggled catalog model keeps its source.
    #[test]
    fn a_connection_offers_its_own_models_then_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap();
        store.create_auth_profile("a1", "Company key", "anthropic", "k", None, None, 0, 1, None, None).unwrap();
        let p = store.get_auth_profile("a1").unwrap().unwrap();
        let key = connection_key(&p);
        store.put_connection_model(&key, "claude-opus-5", "claude-opus-5", "chat", Some(1_000_000), "[\"vision\"]", "added", true).unwrap();
        store.put_connection_model(&key, "jev-latest", "jev-latest", "decision", None, "[]", "added", true).unwrap();
        let catalog: config::ModelsConfig = serde_yaml::from_str(
            "providers:\n  anthropic:\n    - id: claude-haiku-4-5\n      displayName: Claude Haiku 4.5\n      contextWindow: 200000\n      capabilities: [vision, tools]\n    - id: claude-opus-5\n      displayName: Claude Opus 5\n",
        )
        .unwrap();
        let models = connection_models(&store, &catalog, &p);
        let ids: Vec<(&str, &str, &str)> = models.iter().map(|m| (m.model_id.as_str(), m.source.as_str(), m.kind.as_str())).collect();
        assert!(ids.contains(&("claude-opus-5", "added", "chat")), "the own row wins over the catalog: {ids:?}");
        assert!(ids.contains(&("jev-latest", "added", "decision")));
        assert!(ids.contains(&("claude-haiku-4-5", "catalog", "chat")));
        assert_eq!(ids.iter().filter(|(id, _, _)| *id == "claude-opus-5").count(), 1);
    }

    /// OpenRouter's model ids hold a slash (`anthropic/claude-sonnet-5`):
    /// the app sends it encoded (`%2F`) in the path, and the route hands the
    /// handler the decoded id.
    #[tokio::test]
    async fn a_model_id_with_a_slash_reaches_the_handler_whole() {
        use tower::ServiceExt;
        let app = axum::Router::new().route(
            "/providers/{id}/models/{modelId}",
            axum::routing::put(|Path((id, model)): Path<(String, String)>| async move { format!("{id}|{model}") }),
        );
        let req = axum::http::Request::put("/providers/or/models/anthropic%2Fclaude-sonnet-5")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"or|anthropic/claude-sonnet-5");
    }

    #[test]
    fn abilities_keep_only_what_a_row_shows() {
        let caps: Vec<String> = ["streaming", "reasoning", "vision", "tools", "code"].iter().map(|s| s.to_string()).collect();
        assert_eq!(abilities(&caps), vec!["vision", "tools", "thinking"]);
    }
}
