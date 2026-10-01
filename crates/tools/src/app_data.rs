//! An app's own data: the one key-value store its page reads and writes
//! through `nebo.storage`, and `app_data`, the tool its employee reads and
//! writes the same store with.
//!
//! One store, one encoding. The page's `storage.setItem(key, value)` sends
//! the value as text (a string as is, anything else JSON-encoded) and the
//! server keeps that text JSON-encoded (`PUT /apps/{id}/storage/{key}`).
//! [`encode`] writes exactly that and [`decode`] reads it back to the value
//! the page meant, so a contact the employee adds is the contact the page
//! shows, and the other way round.
//!
//! Scope: an employee only ever reaches its own app's store (the app is the
//! employee itself). Another employee asks the app's employee for what it
//! needs (`send_message`, `delegate`); nothing reaches a store directly.
//!
//! Every write tells the app's open views ([`CHANGED_EVENT`]), so a page
//! refreshes when its employee changes what it shows.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::web_tool::Broadcaster;

pub const APP_DATA: &str = "app_data";

/// The WS event an app's open views hear after its store changed; its
/// payload is `{"appId", "keys": [..], "action": "set"|"delete", "source":
/// "employee"|"page"}`. The SDK surfaces it as `storage.onChange(cb)`.
pub const CHANGED_EVENT: &str = "app_data_changed";

/// Records `query` returns by default and at most.
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

/// The name an app's store is kept under.
pub fn store_name(app_id: &str) -> String {
    format!("app:{app_id}")
}

/// One key as stored, or `None` when it is absent (an emptied key, from
/// before deletes removed the row, reads as absent too).
pub fn read(store: &db::Store, app_id: &str, key: &str) -> Result<Option<String>, String> {
    store
        .get_plugin_setting(&store_name(app_id), key)
        .map(|v| v.filter(|raw| !raw.is_empty()))
        .map_err(|e| e.to_string())
}

/// Write one key as stored text (see [`encode`]).
pub fn write(store: &db::Store, app_id: &str, key: &str, raw: &str) -> Result<(), String> {
    let name = store_name(app_id);
    store.ensure_plugin_registry_entry(&name).map_err(|e| e.to_string())?;
    store.set_plugin_setting(&name, key, raw).map_err(|e| e.to_string())
}

/// Remove one key.
pub fn remove(store: &db::Store, app_id: &str, key: &str) -> Result<(), String> {
    store
        .delete_plugin_setting(&store_name(app_id), key)
        .map_err(|e| e.to_string())
}

/// Every key with its stored text, ordered by key.
pub fn list(store: &db::Store, app_id: &str) -> Result<Vec<(String, String)>, String> {
    store
        .list_plugin_settings(&store_name(app_id))
        .map(|items| items.into_iter().filter(|(_, raw)| !raw.is_empty()).collect())
        .map_err(|e| e.to_string())
}

/// The stored text for a value, exactly as the page's `setItem` stores it.
pub fn encode(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    Value::String(text).to_string()
}

/// The value stored text holds, exactly as the page's `getItem` reads it.
pub fn decode(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::String(text)) => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        Ok(other) => other,
        Err(_) => Value::String(raw.to_string()),
    }
}

/// The change event's payload.
pub fn changed(app_id: &str, keys: &[&str], action: &str, source: &str) -> Value {
    json!({ "appId": app_id, "keys": keys, "action": action, "source": source })
}

/// The line an app employee reads about its own data, beside where its
/// files are served from.
pub const SELF_LINE: &str = "Your app's data (what your page keeps with nebo.storage) is yours: read, search and \
change it with app_data. A coworker who needs it asks you.";

/// `app_data` is withheld from every employee that is not an app.
pub fn withheld(store: &db::Store, agent_id: &str) -> Vec<String> {
    if is_app(store, agent_id) {
        Vec::new()
    } else {
        vec![APP_DATA.to_string()]
    }
}

fn is_app(store: &db::Store, agent_id: &str) -> bool {
    !agent_id.is_empty()
        && store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .is_some_and(|a| a.is_app.unwrap_or(0) != 0)
}

/// A field of a record by dotted path (`phone.mobile`).
fn field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, part| match v {
        Value::Object(m) => m.get(part),
        Value::Array(a) => part.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

/// A field's text for a contains-match, any case.
fn lower(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_lowercase(),
        other => other.to_string().to_lowercase(),
    }
}

/// Whether a record matches every `where` condition and the free `text`.
/// A string condition matches when the field contains it (any case); any
/// other condition matches an equal field.
fn matches(record: &Value, conditions: &serde_json::Map<String, Value>, text: &str) -> bool {
    let fields_hold = conditions.iter().all(|(path, want)| match (field(record, path), want) {
        (Some(have), Value::String(w)) => lower(have).contains(&w.to_lowercase()),
        (Some(have), w) => have == w,
        (None, _) => false,
    });
    fields_hold && (text.is_empty() || record.to_string().to_lowercase().contains(text))
}

/// Records a query looks at: each key's value, and each item of a value
/// that is a list (an app often keeps `contacts: [...]` under one key).
pub fn query(
    items: &[(String, String)],
    prefix: &str,
    conditions: &serde_json::Map<String, Value>,
    text: &str,
    limit: usize,
) -> (Vec<Value>, usize) {
    let text = text.to_lowercase();
    let mut found = Vec::new();
    let mut total = 0;
    for (key, raw) in items.iter().filter(|(k, _)| k.starts_with(prefix)) {
        let value = decode(raw);
        let mut take = |hit: Value| {
            total += 1;
            if found.len() < limit {
                found.push(hit);
            }
        };
        match &value {
            Value::Array(list) => {
                for (index, item) in list.iter().enumerate() {
                    if matches(item, conditions, &text) {
                        take(json!({ "key": key, "index": index, "value": item }));
                    }
                }
            }
            other => {
                if matches(other, conditions, &text) {
                    take(json!({ "key": key, "value": other }));
                }
            }
        }
    }
    (found, total)
}

pub struct AppDataTool {
    store: Arc<db::Store>,
    broadcaster: Option<Broadcaster>,
}

impl AppDataTool {
    pub fn new(store: Arc<db::Store>, broadcaster: Option<Broadcaster>) -> Self {
        Self { store, broadcaster }
    }

    /// The calling employee's app: the employee itself, when it is one.
    fn own_app(&self, ctx: &ToolContext) -> Result<db::models::Agent, String> {
        let id = types::keyparser::extract_agent_id(&ctx.session_key);
        match self.store.get_agent(&id).ok().flatten() {
            Some(a) if a.is_app.unwrap_or(0) != 0 => Ok(a),
            _ => Err("app_data reaches only your own app's data, and you are not an app. \
                      Ask the app's employee for what you need (send_message)."
                .to_string()),
        }
    }

    fn notify(&self, app_id: &str, key: &str, action: &str) {
        if let Some(broadcast) = self.broadcaster.as_ref() {
            broadcast(CHANGED_EVENT, changed(app_id, &[key], action, "employee"));
        }
    }

    fn run(&self, app: &db::models::Agent, input: &Value) -> Result<String, String> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let key = input.get("key").and_then(Value::as_str).unwrap_or("").trim();
        let need_key = || {
            if key.is_empty() {
                Err(format!("'{action}' needs a key."))
            } else {
                Ok(())
            }
        };
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);
        let prefix = input.get("prefix").and_then(Value::as_str).unwrap_or("");
        match action {
            "get" => {
                need_key()?;
                Ok(match read(&self.store, &app.id, key)? {
                    Some(raw) => json!({ "key": key, "value": decode(&raw) }).to_string(),
                    None => format!("No key '{key}' in {}'s data.", app.name),
                })
            }
            "set" => {
                need_key()?;
                let value = input.get("value").ok_or("'set' needs a value.")?;
                write(&self.store, &app.id, key, &encode(value))?;
                self.notify(&app.id, key, "set");
                Ok(format!("Saved '{key}'. Open views of {} were told to refresh.", app.name))
            }
            "delete" => {
                need_key()?;
                remove(&self.store, &app.id, key)?;
                self.notify(&app.id, key, "delete");
                Ok(format!("Deleted '{key}'."))
            }
            "list" => {
                let items = list(&self.store, &app.id)?;
                let matching: Vec<_> = items.iter().filter(|(k, _)| k.starts_with(prefix)).collect();
                let shown: Vec<Value> = matching
                    .iter()
                    .take(limit)
                    .map(|(k, raw)| json!({ "key": k, "value": decode(raw) }))
                    .collect();
                Ok(json!({ "total": matching.len(), "items": shown }).to_string())
            }
            "query" => {
                let empty = serde_json::Map::new();
                let conditions = input.get("where").and_then(Value::as_object).unwrap_or(&empty);
                let text = input.get("text").and_then(Value::as_str).unwrap_or("");
                let (found, total) = query(&list(&self.store, &app.id)?, prefix, conditions, text, limit);
                Ok(json!({ "total": total, "items": found }).to_string())
            }
            other => Err(format!("Unknown action '{other}'. Use get, set, delete, list or query.")),
        }
    }
}

type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

impl DynTool for AppDataTool {
    fn name(&self) -> &str {
        APP_DATA
    }

    fn description(&self) -> String {
        "Reads and changes your own app's data: the same key-value store your page reads and writes with \
         nebo.storage (same keys, same values). Actions: get (key), set (key, value: any JSON), delete (key), \
         list (prefix?, limit?), query (prefix?, where: {field: value} — text matches when the field contains it, \
         any case, other values must be equal; dotted paths like \"phone.mobile\" — text?: words anywhere in the \
         record, limit?). A key holding a list is searched item by item. After a set or delete your page's open \
         views are told to refresh. Only your own app: a coworker who needs your data asks you."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set", "delete", "list", "query"] },
                "key": { "type": "string", "description": "The storage key (get, set, delete)." },
                "value": { "description": "The value to store (set): any JSON, the way the page stores it." },
                "prefix": { "type": "string", "description": "Only keys starting with this (list, query)." },
                "where": {
                    "type": "object",
                    "description": "Field conditions, all must hold (query). A string matches when the field contains it, any case.",
                    "additionalProperties": true
                },
                "text": { "type": "string", "description": "Words anywhere in a record (query)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Most records returned (list, query); default 20." }
            },
            "required": ["action"]
        })
    }

    fn search_hint(&self) -> &str {
        "app data storage records lookup save"
    }

    fn read_only(&self, input: &Value) -> bool {
        matches!(
            input.get("action").and_then(Value::as_str),
            Some("get" | "list" | "query")
        )
    }

    fn activity(&self, input: &Value) -> String {
        let key = input.get("key").and_then(Value::as_str).unwrap_or("");
        match input.get("action").and_then(Value::as_str).unwrap_or("") {
            "set" => format!("saving {key}"),
            "delete" => format!("deleting {key}"),
            "get" => format!("reading {key}"),
            _ => "looking through the app's data".to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        let key = input.get("key").and_then(Value::as_str).unwrap_or("");
        match input.get("action").and_then(Value::as_str).unwrap_or("") {
            "set" => format!("Saved {key}"),
            "delete" => format!("Deleted {key}"),
            "get" => format!("Read {key}"),
            _ => "Looked through the app's data".to_string(),
        }
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move {
            let app = match self.own_app(ctx) {
                Ok(a) => a,
                Err(e) => return ToolResult::error(e),
            };
            match self.run(&app, &input) {
                Ok(text) => ToolResult::ok(text),
                Err(e) => ToolResult::error(e),
            }
        })
    }
}

#[cfg(test)]
mod tests;
