//! A provider as a connection: one key the owner added, and the models it
//! offers — models typed in by hand, and models picked while browsing the
//! provider's own list. Nebo names no model for a connection (owner,
//! 2026-10-11). A model is a chat model or a decision model
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
/// The platform fee on a request made on the owner's own key, in USD, when
/// Janus's synced list states none (a bot not yet synced): the price book's
/// `byok_fee_micro` (150 µ$). Janus's synced figure wins when present.
pub const OWN_KEY_REQUEST_FEE: &str = "0.00015";
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
    /// What the owner reads about it and what it costs on this connection
    /// (set when the list is shown; never stored with the model).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<ModelCard>,
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
        card: None,
    }
}

/// Every model a connection offers: the ones the owner typed or picked
/// while browsing, and nothing Nebo chose for him.
pub fn connection_models(store: &db::Store, profile: &db::models::AuthProfile) -> Vec<ConnectionModel> {
    store.list_provider_models(&connection_key(profile)).unwrap_or_default().iter().map(model_of_row).collect()
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
    let via_janus = auth::credential::own_key(&p).is_some();
    let models: Vec<ConnectionModel> = connection_models(&state.store, &p)
        .into_iter()
        .map(|m| carded(&state.store, m, via_janus))
        .collect();
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
        card: None,
        model_id,
    };
    put(&state, &connection_key(&p), &model).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "model": model })))
}

/// PUT /api/v1/providers/{id}/models/{modelId} — turn a model on or off, or
/// change its kind, context or abilities.
pub async fn update_connection_model(
    State(state): State<AppState>,
    Path((id, model_id)): Path<(String, String)>,
    Json(body): Json<ModelBody>,
) -> HandlerResult<serde_json::Value> {
    let p = profile(&state, &id).map_err(to_error_response)?;
    let mut model = connection_models(&state.store, &p)
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
/// connection added.
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

// ── Model cards ────────────────────────────────────────────────────

/// What the owner reads about a model before he picks it: Janus's word on
/// it (GET /v1/models; the registry is the one source), stored with the
/// synced row (`provider_models.card`). A connection's model reads the card
/// of the same model in Janus's list when Janus knows it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelCard {
    pub display_name: String,
    pub family: String,
    pub maker: String,
    /// What it is good for, in a sentence or two.
    pub description: String,
    /// The Intelligence level it fits: instant, low, medium, high, max; "" unknown.
    pub tier: String,
    pub reasons: bool,
    pub abilities: CardAbilities,
    pub context_window: Option<i64>,
    pub max_output: Option<i64>,
    /// fastest, fast, standard, slow; "" unknown.
    pub speed: String,
    pub pricing: Option<CardPricing>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardAbilities {
    pub vision: bool,
    pub tools: bool,
    pub reasoning: bool,
    pub audio: bool,
}

/// A model's customer price (USD decimal strings). Nebo AI's models carry
/// the price book's retail rates per 1M tokens and the request fee; a model
/// on the owner's own key only the request fee, its provider billing him
/// for the model (`provider_bills`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardPricing {
    pub version: String,
    pub currency: String,
    pub input_per1m: Option<String>,
    pub output_per1m: Option<String>,
    pub cache_read_per1m: Option<String>,
    pub cache_write_per1m: Option<String>,
    pub request_fee: Option<String>,
    pub provider_bills: bool,
    /// The retail rates by the request's input size, ascending: a band
    /// applies up to its `up_to_context` tokens, the last (open) one above
    /// the others. One band for a model priced the same at every size; the
    /// flat fields above are the first band. Empty on the owner's own key.
    pub bands: Vec<CardBand>,
}

/// One price band of a model priced by the size of its input.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardBand {
    /// The band's top, in input tokens; None for the open top band.
    pub up_to_context: Option<i64>,
    pub input_per1m: Option<String>,
    pub output_per1m: Option<String>,
    pub cache_read_per1m: Option<String>,
    pub cache_write_per1m: Option<String>,
}

impl CardAbilities {
    /// The abilities as a model row lists them (`thinking` for reasoning).
    pub fn list(&self) -> Vec<String> {
        [(self.vision, "vision"), (self.tools, "tools"), (self.reasoning, "thinking")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, a)| a.to_string())
            .collect()
    }
}

/// Whether a model row is one of the models Janus's catalog lists (it carries
/// a card), not nebo-1 or a speed: the ONE rule for a catalog row. Payloads
/// mark such rows `catalog: true`, and they are chosen only by their full id.
pub fn is_catalog_row(row: &db::models::ProviderModel) -> bool {
    row.provider == "janus" && row.card.is_some()
}

/// A row's card, when it has one.
pub fn card_of_row(row: &db::models::ProviderModel) -> Option<ModelCard> {
    row.card.as_deref().and_then(|c| serde_json::from_str(c).ok())
}

/// The card Janus gives `model_id`, when its list holds the model (an
/// OpenRouter id `anthropic/claude-x` is the same model as `claude-x`).
pub fn janus_card(store: &db::Store, model_id: &str) -> Option<ModelCard> {
    let rows = store.list_provider_models("janus").ok()?;
    let bare = model_id.rsplit_once('/').map_or(model_id, |(_, m)| m);
    rows.iter().find(|r| r.model_id == model_id).or_else(|| rows.iter().find(|r| r.model_id == bare)).and_then(card_of_row)
}

/// The request fee Janus's list states (the price book in effect), when it
/// states one.
pub fn janus_request_fee(store: &db::Store) -> Option<String> {
    store
        .list_provider_models("janus")
        .ok()?
        .iter()
        .filter_map(card_of_row)
        .find_map(|c| c.pricing.and_then(|p| p.request_fee))
}

/// The card of a model a connection offers. On the owner's own key through
/// Janus (`via_janus`) it costs the request fee and its provider bills the
/// model; on a URL he entered it goes direct and Nebo bills nothing. What it
/// is comes from Janus's card for the same model when Janus knows it, else
/// from what the connection knows (its name, window and abilities).
pub fn connection_card(store: &db::Store, m: &ConnectionModel, via_janus: bool) -> ModelCard {
    let mut card = janus_card(store, &m.model_id).unwrap_or_else(|| ModelCard {
        display_name: m.display_name.clone(),
        family: family_of(&m.display_name.replace(['-', '_'], " ")),
        context_window: m.context_window,
        abilities: CardAbilities {
            vision: m.capabilities.iter().any(|c| c == "vision"),
            tools: m.capabilities.iter().any(|c| c == "tools"),
            reasoning: m.capabilities.iter().any(|c| c == "thinking"),
            audio: false,
        },
        reasons: m.capabilities.iter().any(|c| c == "thinking"),
        ..ModelCard::default()
    });
    card.pricing = Some(CardPricing {
        currency: "USD".into(),
        request_fee: via_janus.then(|| janus_request_fee(store).unwrap_or_else(|| OWN_KEY_REQUEST_FEE.to_string())),
        provider_bills: true,
        ..CardPricing::default()
    });
    card
}

/// A connection's model with its card, and with the card's abilities when
/// the connection names none, so what the card says and what a pack slot
/// may take (Vision) agree.
pub fn carded(store: &db::Store, mut m: ConnectionModel, via_janus: bool) -> ConnectionModel {
    let card = connection_card(store, &m, via_janus);
    if m.capabilities.is_empty() {
        m.capabilities = card.abilities.list();
    }
    m.card = Some(card);
    m
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
    /// What Janus knows of the model and what it costs on this connection.
    pub card: Option<ModelCard>,
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
                card: None,
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
                card: None,
            })
        })
        .collect()
}

/// A connection's fetched list, kept for [`CATALOG_TTL`] with the
/// fingerprint of the key and URL it was fetched with: a new key or URL
/// fetches again at once.
type CachedList = (Instant, [u8; 32], Vec<CatalogModel>);
fn cached() -> &'static Mutex<HashMap<String, CachedList>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, CachedList>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The fingerprint of what a listing call is made with (its URL and key):
/// a list fetched with another key or URL is not this one's.
fn list_fingerprint(r: &CatalogRequest) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(r.url.as_bytes());
    if let Some((k, v)) = &r.auth {
        h.update([0]);
        h.update(k.as_bytes());
        h.update([0]);
        h.update(v.as_bytes());
    }
    h.finalize().into()
}

/// Anthropic's `/v1/models`: ids, names and when each came out.
pub fn parse_anthropic_list(body: &serde_json::Value) -> Vec<CatalogModel> {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            let name = m["display_name"].as_str().unwrap_or(&id).to_string();
            let created = m["created_at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp());
            Some(CatalogModel {
                family: family_of(&name),
                display_name: name,
                model_id: id,
                kind: "chat".into(),
                context_window: None,
                capabilities: Vec::new(),
                pricing: None,
                created,
                added: false,
                card: None,
            })
        })
        .collect()
}

/// Google's `models` list: the models that write (generateContent), with
/// their input window.
pub fn parse_google_list(body: &serde_json::Value) -> Vec<CatalogModel> {
    body["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent"))))
        .filter_map(|m| {
            let id = m["name"].as_str()?.trim_start_matches("models/").to_string();
            let name = m["displayName"].as_str().unwrap_or(&id).to_string();
            Some(CatalogModel {
                family: family_of(&name),
                display_name: name,
                model_id: id,
                kind: "chat".into(),
                context_window: m["inputTokenLimit"].as_i64(),
                capabilities: Vec::new(),
                pricing: None,
                created: None,
                added: false,
                card: None,
            })
        })
        .collect()
}

/// How a connection's provider lists its models: the URL, the key's header
/// and the shape of the answer. A direct, unbilled call on the owner's own
/// key (the list is not usage). A connection with a URL the owner entered
/// (`auth::credential::direct_url`, the one test) is asked at THAT URL, as an
/// OpenAI-compatible `/models`, whatever its kind: its key never goes to a
/// provider's own host.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogRequest {
    pub url: String,
    /// (header, value): the key as the provider takes it; None for a public list.
    pub auth: Option<(String, String)>,
    pub extra: Vec<(String, String)>,
    pub shape: CatalogShape,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CatalogShape {
    OpenRouter,
    OpenAi,
    Anthropic,
    Google,
}

/// The listing call for a connection, or None when its provider has none.
pub fn catalog_request(p: &db::models::AuthProfile) -> Option<CatalogRequest> {
    let key = auth::credential::profile_key(p);
    let bearer = || Some(("Authorization".to_string(), format!("Bearer {key}")));
    let req = |url: String, auth, extra, shape| Some(CatalogRequest { url, auth, extra, shape });
    match auth::credential::direct_url(p) {
        // His own URL: listed there, or not at all (a provider whose format
        // Nebo has no direct client for is neither listed nor used).
        Some(Ok(base)) => {
            return match p.provider.as_str() {
                "systemone_compatible" | "ollama" | "neboai" => None,
                _ => req(format!("{}/models", base.trim_end_matches('/')), bearer(), vec![], CatalogShape::OpenAi),
            };
        }
        Some(Err(_)) => return None,
        None => {}
    }
    match p.provider.as_str() {
        "openrouter" => req("https://openrouter.ai/api/v1/models".into(), None, vec![], CatalogShape::OpenRouter),
        "anthropic" => req(
            "https://api.anthropic.com/v1/models?limit=1000".into(),
            Some(("x-api-key".into(), key.clone())),
            vec![("anthropic-version".into(), "2023-06-01".into())],
            CatalogShape::Anthropic,
        ),
        "google" => req(
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000".into(),
            Some(("x-goog-api-key".into(), key.clone())),
            vec![],
            CatalogShape::Google,
        ),
        "openai" => req("https://api.openai.com/v1/models".into(), bearer(), vec![], CatalogShape::OpenAi),
        "deepseek" => req("https://api.deepseek.com/v1/models".into(), bearer(), vec![], CatalogShape::OpenAi),
        "xai" => req("https://api.x.ai/v1/models".into(), bearer(), vec![], CatalogShape::OpenAi),
        _ => None,
    }
}

/// Parse a provider's list by its shape.
pub fn parse_catalog(shape: CatalogShape, body: &serde_json::Value) -> Vec<CatalogModel> {
    match shape {
        CatalogShape::OpenRouter => parse_openrouter(body),
        CatalogShape::OpenAi => parse_openai_list(body),
        CatalogShape::Anthropic => parse_anthropic_list(body),
        CatalogShape::Google => parse_google_list(body),
    }
}

/// Why a provider's list could not be read: the owner's to fix (his key
/// refused), the provider's to clear (too many requests), or the way there
/// (unreachable, too slow, an answer that is not a list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// 401 or 403: the key was refused.
    Key,
    /// 429: the provider asks to wait.
    RateLimited,
    /// No answer in time, no connection, or an answer that is not a list.
    Unreachable,
}

impl CatalogError {
    /// The code the app words in the owner's language.
    pub fn code(self) -> &'static str {
        match self {
            CatalogError::Key => "key",
            CatalogError::RateLimited => "rate_limited",
            CatalogError::Unreachable => "unreachable",
        }
    }
}

/// The provider's own list (`catalog_request`): `Ok(None)` when it has no
/// list, the reason when the call failed. A list is kept for a day for the
/// key and URL it was fetched with (`list_fingerprint`); a failure is never
/// kept.
pub(crate) async fn fetch_catalog(p: &db::models::AuthProfile) -> Result<Option<Vec<CatalogModel>>, CatalogError> {
    let Some(r) = catalog_request(p) else {
        return Ok(None);
    };
    let fingerprint = list_fingerprint(&r);
    if let Some((at, fp, list)) = cached().lock().ok().and_then(|c| c.get(&p.id).cloned())
        && fp == fingerprint
        && at.elapsed() < CATALOG_TTL
    {
        return Ok(Some(list));
    }
    let list = fetch_list(&r).await?;
    if let Ok(mut c) = cached().lock() {
        c.insert(p.id.clone(), (Instant::now(), fingerprint, list.clone()));
    }
    Ok(Some(list))
}

/// What a listing answer's status means.
pub(crate) fn catalog_status(status: u16) -> Result<(), CatalogError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(CatalogError::Key),
        429 => Err(CatalogError::RateLimited),
        _ => Err(CatalogError::Unreachable),
    }
}

/// Make one listing call and parse its answer.
pub(crate) async fn fetch_list(r: &CatalogRequest) -> Result<Vec<CatalogModel>, CatalogError> {
    let client = tls::http_client().timeout(Duration::from_secs(15)).build().map_err(|_| CatalogError::Unreachable)?;
    let mut req = client.get(&r.url);
    for (h, v) in r.auth.iter().chain(r.extra.iter()) {
        req = req.header(h.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|_| CatalogError::Unreachable)?;
    catalog_status(resp.status().as_u16())?;
    let body = resp.json().await.map_err(|_| CatalogError::Unreachable)?;
    Ok(parse_catalog(r.shape, &body))
}

/// Each listed model with what Janus knows of it (its card, and its
/// abilities when the provider's list names none).
pub fn with_cards(store: &db::Store, list: Vec<CatalogModel>, via_janus: bool) -> Vec<CatalogModel> {
    list.into_iter()
        .map(|mut m| {
            let row = ConnectionModel {
                model_id: m.model_id.clone(),
                display_name: m.display_name.clone(),
                kind: m.kind.clone(),
                context_window: m.context_window,
                capabilities: m.capabilities.clone(),
                source: "browsed".into(),
                is_active: false,
                card: None,
            };
            let card = connection_card(store, &row, via_janus);
            if m.capabilities.is_empty() {
                m.capabilities = card.abilities.list();
            }
            if m.context_window.is_none() {
                m.context_window = card.context_window;
            }
            m.card = Some(card);
            m
        })
        .collect()
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
    let list = match fetch_catalog(&p).await {
        Ok(Some(list)) => list,
        Ok(None) => return Ok(Json(serde_json::json!({ "browsable": false, "total": 0, "models": [] }))),
        // The list is there to browse; this call could not read it. The app
        // says why and offers to try again.
        Err(e) => return Ok(Json(serde_json::json!({ "browsable": true, "total": 0, "models": [], "error": e.code() }))),
    };
    let have: Vec<String> = connection_models(&state.store, &p)
        .into_iter()
        .map(|m| m.model_id)
        .collect();
    let list = with_cards(&state.store, list, auth::credential::own_key(&p).is_some());
    let models = filter_catalog(&list, &q, &have);
    let refreshed_at = cached()
        .lock()
        .ok()
        .and_then(|c| c.get(&p.id).map(|(at, _, _)| chrono::Utc::now().timestamp() - at.elapsed().as_secs() as i64));
    Ok(Json(serde_json::json!({ "browsable": true, "total": list.len(), "refreshedAt": refreshed_at, "models": models, "error": null })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_of(provider: &str, base_url: Option<&str>) -> db::models::AuthProfile {
        let store = db::Store::new(":memory:").unwrap();
        store.create_auth_profile("p", "P", provider, "sk-test", None, base_url, 0, 1, None, None).unwrap()
    }

    /// Each built-in provider lists its models with the owner's key as it
    /// takes it, on its own API; a URL the owner entered is asked directly.
    #[test]
    fn each_provider_lists_its_models_its_own_way() {
        let r = catalog_request(&profile_of("anthropic", None)).unwrap();
        assert!(r.url.starts_with("https://api.anthropic.com/v1/models"));
        assert_eq!(r.auth, Some(("x-api-key".into(), "sk-test".into())));
        assert!(r.extra.contains(&("anthropic-version".into(), "2023-06-01".into())));
        assert_eq!(r.shape, CatalogShape::Anthropic);
        let r = catalog_request(&profile_of("google", None)).unwrap();
        assert!(r.url.starts_with("https://generativelanguage.googleapis.com/v1beta/models"));
        assert_eq!(r.auth, Some(("x-goog-api-key".into(), "sk-test".into())));
        for (kind, url) in [
            ("openai", "https://api.openai.com/v1/models"),
            ("deepseek", "https://api.deepseek.com/v1/models"),
            ("xai", "https://api.x.ai/v1/models"),
        ] {
            let r = catalog_request(&profile_of(kind, None)).unwrap();
            assert_eq!((r.url.as_str(), r.shape), (url, CatalogShape::OpenAi), "{kind}");
            assert_eq!(r.auth, Some(("Authorization".into(), "Bearer sk-test".into())));
        }
        let r = catalog_request(&profile_of("openrouter", None)).unwrap();
        assert_eq!((r.url.as_str(), r.auth.is_none()), ("https://openrouter.ai/api/v1/models", true));
        let r = catalog_request(&profile_of("openai_compatible", Some("https://lab.example/v1/"))).unwrap();
        assert_eq!(r.url, "https://lab.example/v1/models");
        assert!(catalog_request(&profile_of("openai_compatible", None)).is_none());
        assert!(catalog_request(&profile_of("systemone_compatible", Some("https://x"))).is_none());
    }

    /// A connection with a URL the owner entered lists from THAT URL, as an
    /// OpenAI-compatible `/models`: its key never goes to a provider's own
    /// host. Anthropic and Google at a URL of his are neither listed nor
    /// used (`auth::credential::direct_url`): chat has no client for them.
    #[test]
    fn an_owner_url_lists_from_that_url_only() {
        for kind in ["anthropic", "google"] {
            assert!(catalog_request(&profile_of(kind, Some("https://gateway.example/v1"))).is_none(), "{kind}: not listed");
        }
        for kind in ["openai", "deepseek", "xai", "openrouter", "openai_compatible"] {
            let r = catalog_request(&profile_of(kind, Some("https://gateway.example/v1"))).unwrap();
            assert_eq!(r.url, "https://gateway.example/v1/models", "{kind}");
            assert_eq!(r.shape, CatalogShape::OpenAi, "{kind}");
            assert_eq!(r.auth, Some(("Authorization".into(), "Bearer sk-test".into())), "{kind}");
            for host in ["api.anthropic.com", "googleapis.com", "api.openai.com", "api.deepseek.com", "api.x.ai", "openrouter.ai"] {
                assert!(!r.url.contains(host), "{kind}: the key never goes to {host}");
            }
        }
    }

    /// A listing that cannot be read says why: the key refused, the provider
    /// asking to wait, or no answer.
    #[test]
    fn a_failed_listing_says_why() {
        assert_eq!(catalog_status(200), Ok(()));
        assert_eq!(catalog_status(401), Err(CatalogError::Key));
        assert_eq!(catalog_status(403), Err(CatalogError::Key));
        assert_eq!(catalog_status(429), Err(CatalogError::RateLimited));
        assert_eq!(catalog_status(500), Err(CatalogError::Unreachable));
        assert_eq!(CatalogError::Key.code(), "key");
    }

    /// A fake provider answering its list with `status` and counting calls.
    async fn fake_list(status: u16) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(move || {
                let n = n.clone();
                async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        Json(serde_json::json!({ "data": [{ "id": "lab-1" }] })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/v1"), calls)
    }

    /// A list is kept for the key and URL it was fetched with: a new key
    /// fetches again at once; a refused key is reported, never kept.
    #[tokio::test]
    async fn a_new_key_fetches_the_list_again_and_a_refusal_is_reported() {
        use std::sync::atomic::Ordering;
        let (base, calls) = fake_list(200).await;
        let store = db::Store::new(":memory:").unwrap();
        let p = store.create_auth_profile("cache-test", "Lab", "openai_compatible", "sk-one", None, Some(&base), 0, 1, None, None).unwrap();
        assert_eq!(fetch_catalog(&p).await.unwrap().unwrap()[0].model_id, "lab-1");
        fetch_catalog(&p).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "kept for the same key");
        let mut changed = p.clone();
        changed.api_key = "sk-two".into();
        fetch_catalog(&changed).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a new key fetches again");

        let (refusing, _) = fake_list(401).await;
        let r = store.create_auth_profile("refused-test", "Lab", "openai_compatible", "sk-bad", None, Some(&refusing), 0, 1, None, None).unwrap();
        assert_eq!(fetch_catalog(&r).await, Err(CatalogError::Key));
        let (busy, _) = fake_list(429).await;
        let b = store.create_auth_profile("busy-test", "Lab", "openai_compatible", "sk-x", None, Some(&busy), 0, 1, None, None).unwrap();
        assert_eq!(fetch_catalog(&b).await, Err(CatalogError::RateLimited));
        let gone = store.create_auth_profile("gone-test", "Lab", "openai_compatible", "sk-x", None, Some("http://127.0.0.1:9/v1"), 0, 1, None, None).unwrap();
        assert_eq!(fetch_catalog(&gone).await, Err(CatalogError::Unreachable));
    }

    /// The own-key fee is always shown: Janus's synced figure, else Nebo's
    /// own (a bot not yet synced); a URL the owner entered costs Nebo nothing.
    #[test]
    fn the_own_key_fee_is_always_shown() {
        let store = db::Store::new(":memory:").unwrap();
        let m = ConnectionModel {
            model_id: "claude-x".into(),
            display_name: "claude-x".into(),
            kind: "chat".into(),
            context_window: None,
            capabilities: vec![],
            source: "added".into(),
            is_active: true,
            card: None,
        };
        let p = connection_card(&store, &m, true).pricing.unwrap();
        assert_eq!((p.request_fee.as_deref(), p.provider_bills), (Some(OWN_KEY_REQUEST_FEE), true));
        assert_eq!(connection_card(&store, &m, false).pricing.unwrap().request_fee, None);
    }

    /// A model the connection names no abilities for takes the card's, so
    /// the card and the Vision slot agree.
    #[test]
    fn a_model_without_abilities_takes_the_cards() {
        let store = db::Store::new(":memory:").unwrap();
        store.upsert_provider_model("janus/claude-sonnet-5", "janus", "claude-sonnet-5", "Claude Sonnet 5", None, None, None, None, None, None, None, true).unwrap();
        let card = ModelCard { abilities: CardAbilities { vision: true, tools: true, reasoning: false, audio: false }, ..ModelCard::default() };
        store.set_provider_model_card("janus/claude-sonnet-5", Some(&serde_json::to_string(&card).unwrap())).unwrap();
        let m = ConnectionModel {
            model_id: "claude-sonnet-5".into(),
            display_name: "claude-sonnet-5".into(),
            kind: "chat".into(),
            context_window: None,
            capabilities: vec![],
            source: "added".into(),
            is_active: true,
            card: None,
        };
        assert_eq!(carded(&store, m, true).capabilities, ["vision", "tools"]);
    }

    #[test]
    fn anthropic_and_google_lists_parse() {
        let a = parse_anthropic_list(&serde_json::json!({ "data": [
            { "type": "model", "id": "claude-sonnet-5", "display_name": "Claude Sonnet 5", "created_at": "2026-05-01T00:00:00Z" }
        ]}));
        assert_eq!((a[0].model_id.as_str(), a[0].display_name.as_str(), a[0].family.as_str()), ("claude-sonnet-5", "Claude Sonnet 5", "Claude Sonnet"));
        assert!(a[0].created.is_some());
        let g = parse_google_list(&serde_json::json!({ "models": [
            { "name": "models/gemini-3.1-pro", "displayName": "Gemini 3.1 Pro", "inputTokenLimit": 1048576,
              "supportedGenerationMethods": ["generateContent", "countTokens"] },
            { "name": "models/text-embedding-005", "displayName": "Embedding", "supportedGenerationMethods": ["embedContent"] }
        ]}));
        assert_eq!(g.len(), 1, "only the models that write");
        assert_eq!((g[0].model_id.as_str(), g[0].context_window), ("gemini-3.1-pro", Some(1_048_576)));
    }

    /// A listed model Janus knows reads Janus's card and abilities; on the
    /// owner's own key it costs the request fee and his provider bills it.
    #[test]
    fn a_listed_model_reads_what_janus_knows_of_it() {
        let store = db::Store::new(":memory:").unwrap();
        store.upsert_provider_model("janus/claude-sonnet-5", "janus", "claude-sonnet-5", "Claude Sonnet 5", None, None, None, None, None, None, None, true).unwrap();
        let card = ModelCard {
            display_name: "Claude Sonnet 5".into(),
            description: "Everyday work.".into(),
            abilities: CardAbilities { vision: true, tools: true, reasoning: true, audio: false },
            context_window: Some(1_000_000),
            pricing: Some(CardPricing { request_fee: Some("0.00015".into()), input_per1m: Some("3".into()), ..CardPricing::default() }),
            ..ModelCard::default()
        };
        store.set_provider_model_card("janus/claude-sonnet-5", Some(&serde_json::to_string(&card).unwrap())).unwrap();
        let listed = parse_anthropic_list(&serde_json::json!({ "data": [
            { "id": "claude-sonnet-5", "display_name": "Claude Sonnet 5" },
            { "id": "claude-unknown-1", "display_name": "Claude Unknown 1" }
        ]}));
        let out = with_cards(&store, listed, true);
        assert_eq!(out[0].capabilities, ["vision", "tools", "thinking"]);
        assert_eq!(out[0].context_window, Some(1_000_000));
        let c = out[0].card.as_ref().unwrap();
        assert_eq!(c.description, "Everyday work.");
        let p = c.pricing.as_ref().unwrap();
        assert_eq!((p.request_fee.as_deref(), p.input_per1m.as_deref(), p.provider_bills), (Some("0.00015"), None, true));
        let unknown = out[1].card.as_ref().unwrap();
        assert_eq!(unknown.display_name, "Claude Unknown 1");
        assert!(unknown.description.is_empty() && out[1].capabilities.is_empty());
        assert_eq!(with_cards(&store, parse_anthropic_list(&serde_json::json!({"data":[{"id":"claude-sonnet-5"}]})), false)[0]
            .card.as_ref().unwrap().pricing.as_ref().unwrap().request_fee, None, "direct: no fee");
    }

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

    /// A connection's models are the ones the owner added, and nothing else.
    #[test]
    fn a_connection_offers_only_the_models_the_owner_added() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap();
        store.create_auth_profile("a1", "Company key", "anthropic", "k", None, None, 0, 1, None, None).unwrap();
        let p = store.get_auth_profile("a1").unwrap().unwrap();
        let key = connection_key(&p);
        assert!(connection_models(&store, &p).is_empty(), "nothing until the owner adds a model");
        store.put_connection_model(&key, "model-a", "model-a", "chat", Some(1_000_000), "[\"vision\"]", "added", true).unwrap();
        store.put_connection_model(&key, "decider-b", "decider-b", "decision", None, "[]", "browsed", true).unwrap();
        let models = connection_models(&store, &p);
        let ids: Vec<(&str, &str, &str)> = models.iter().map(|m| (m.model_id.as_str(), m.source.as_str(), m.kind.as_str())).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(ids.contains(&("model-a", "added", "chat")));
        assert!(ids.contains(&("decider-b", "browsed", "decision")));
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
