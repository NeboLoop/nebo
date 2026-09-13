//! The update run: each seat reads a layer change once and writes its own
//! context section (Playbook PRD v0.7 §6.8, implementation PRD R15).
//!
//! A seat meets its industry, franchise, and company layers the way a new
//! hire meets the handbook: it reads it once, decides what matters to its
//! job, and works from what it wrote down. Nothing about the layers is
//! fetched at work time. This module is the trigger and the run; the seat's
//! `rules` tool is the write; `build_static` is the read.
//!
//! Outside the seat's judgment, and therefore not here: laws' ceilings enter
//! the policy stack at load, and resolved values are written into the seat's
//! inputs. A failed run leaves the previous section in place and marks the
//! stamp stale; it never blocks work.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use crate::chat_dispatch::{run_chat, ChatConfig};
use crate::state::AppState;

/// The seats that read a change run in parallel, but not all at once: a
/// company of forty-eight seats on a pack update is forty-eight short runs,
/// and the provider should see a few at a time.
const PARALLEL_SEATS: usize = 3;

/// Which layer changed, what it is now, and what it was written against.
#[derive(Debug, Clone)]
pub struct LayerChange {
    /// `industry` | `franchise` | `company` | `facts`.
    pub layer: String,
    /// What the seat's section will be stamped with, e.g. `industry:<slug>@<version>`.
    pub stamp: String,
    /// The whole layer on first hire, the diff on an update. What the seat reads.
    pub text: String,
    /// For `facts`: the fact domain that changed (`finance.ar`), so only the
    /// seats that subscribe to it read it. `None` = every seat reads it.
    pub domain: Option<String>,
}

/// Raise `layers_changed`: every seat that reads this layer gets one update
/// run, in the background. Returns at once.
pub fn spawn_layers_changed(state: AppState, change: LayerChange) {
    // The company event (R6): any seat binding `layers_changed` hears it,
    // bare name, no producer prefix.
    workflow::events::emit_company_event(
        "layers_changed",
        serde_json::json!({ "layer": change.layer, "stamp": change.stamp, "domain": change.domain }),
        "",
    );
    tokio::spawn(async move {
        run_for_seats(&state, change).await;
    });
}

/// The seats that read a change: every enabled employee for a pack layer;
/// for a fact domain, the seats whose `subscribes` cover it.
pub fn seats_for(state: &AppState, change: &LayerChange) -> Vec<db::models::Agent> {
    let all = state.store.list_agents(10_000, 0).unwrap_or_default();
    all.into_iter()
        .filter(|a| a.is_enabled == 1 && a.is_app.unwrap_or(0) == 0)
        .filter(|a| match &change.domain {
            None => true,
            Some(domain) => subscribes(&a.frontmatter, domain),
        })
        .collect()
}

/// `subscribes: ["finance.ar.*", "parties.carriers"]` covers a domain when a
/// pattern equals it, is a prefix ending in `.*`, or is `*`.
pub fn subscribes(frontmatter: &str, domain: &str) -> bool {
    let fm: serde_json::Value = serde_json::from_str(frontmatter).unwrap_or_default();
    let Some(list) = fm.get("subscribes").and_then(|v| v.as_array()) else {
        return false;
    };
    list.iter().filter_map(|v| v.as_str()).any(|pattern| {
        if pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix(".*") {
            return domain == prefix || domain.starts_with(&format!("{prefix}."));
        }
        pattern == domain
    })
}

async fn run_for_seats(state: &AppState, change: LayerChange) {
    let seats = seats_for(state, &change);
    if seats.is_empty() {
        info!(layer = %change.layer, stamp = %change.stamp, "layers_changed: no seat reads this layer");
        return;
    }
    info!(layer = %change.layer, stamp = %change.stamp, seats = seats.len(), "layers_changed: update runs");
    let limit = Arc::new(tokio::sync::Semaphore::new(PARALLEL_SEATS));
    let change = Arc::new(change);
    let mut handles = Vec::with_capacity(seats.len());
    for seat in seats {
        let permit = limit.clone().acquire_owned().await.expect("semaphore");
        let state = state.clone();
        let change = change.clone();
        handles.push(tokio::spawn(async move {
            run_for_seat(&state, &seat, &change).await;
            drop(permit);
        }));
    }
    for h in handles {
        let _ = h.await;
    }
}

/// One seat's update run. The stamp is written `pending` before the run and
/// flipped to `written` by the seat's `context write`; a run that ends
/// without a write leaves it `pending`, which the seat page shows as stale.
async fn run_for_seat(state: &AppState, seat: &db::models::Agent, change: &LayerChange) {
    let pending = serde_json::json!({
        "layer": change.layer,
        "against": change.stamp,
        "status": "pending",
        "launched_at": chrono::Utc::now().timestamp(),
    });
    if let Err(e) = state.store.set_agent_context_stamp(&seat.id, &pending.to_string()) {
        warn!(agent = %seat.id, error = %e, "layers_changed: could not stamp");
    }
    let session_key = format!("agent:{}:layers", seat.id);
    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", &seat.id);
    let config = ChatConfig {
        session_key: session_key.clone(),
        prompt: update_prompt(seat, change),
        system: String::new(),
        user_id: String::new(),
        channel: "layers".to_string(),
        origin: tools::Origin::System,
        agent_id: seat.id.clone(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::COMM.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        entity_name: seat.name.clone(),
        origin_agent_id: None,
        mention_context: None,
        tool_scope: None,
        plan_mode: false,
        channel_ctx: None,
        handoff_depth: 0,
        seed_taint: vec![],
        tool_allowlist: None,
        hidden_prompt: true,
        audience: None,
        cwd: None,
        model_override: None,
    };
    run_chat(state, config).await;

    // Did the seat write? If not, the stamp stays pending: stale, never blocking.
    match state.store.get_agent(&seat.id) {
        Ok(Some(a)) => {
            let status = a
                .context_stamp
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(String::from))
                .unwrap_or_default();
            if status == "written" {
                info!(agent = %seat.name, against = %change.stamp, "package part of the rules written");
            } else {
                warn!(agent = %seat.name, against = %change.stamp, "update run ended without a rules write; previous package part kept, marked stale");
            }
        }
        _ => {}
    }
}

fn update_prompt(seat: &db::models::Agent, change: &LayerChange) -> String {
    let rules = seat.rules.as_deref().map(str::trim).unwrap_or("");
    let current = tools::rules_tool::package_block(rules)
        .filter(|s| !s.is_empty())
        .unwrap_or("(none yet)");
    let owners = if rules.is_empty() { "(none)" } else { rules };
    format!(
        "[Update run — not an owner message]\n\
A package your company works by has changed: {layer} ({stamp}).\n\n\
Read it below. Decide what in it matters to YOUR job: your seat, your workflows, the operations you perform, the parties you deal with, the words this company uses. Where the company package and the industry package both speak to something that applies to you, the company's version wins — apply that yourself, item by item. Then call the `rules` tool with action \"write\" and the WHOLE package part of your Rules you will work by from now on: the facts and rules that matter to you, in your own words, in markdown, as short as it can be and no shorter. Keep what still holds from your current part, drop what this change retires, and say what changed.\n\n\
Your Rules as a whole are below. What the owner wrote outside the package markers is theirs: it stays as written, you do not restate it, and your part must not contradict it. Two things are not yours to decide and must not appear as rules: which operations you may perform (laws and your ceiling set that), and the exact values of answered questions (those are in your configured inputs). Do not do any other work in this run. Do not ask questions; if something is unclear, say so inside your part.\n\n\
## Your current package part\n\n{current}\n\n\
## Your Rules today, whole\n\n{owners}\n\n\
## The package\n\n{text}\n",
        layer = change.layer,
        stamp = change.stamp,
        current = current,
        owners = owners,
        text = change.text,
    )
}

/// Boot and pack-watcher entry: compute what changed per pack against the
/// previous set and raise one change per pack that is new or different.
/// `previous` is replaced with `current` on return.
pub fn diff_and_raise(
    state: &AppState,
    previous: &mut HashMap<String, napp::Pack>,
    current: Vec<napp::Pack>,
) {
    let mut next: HashMap<String, napp::Pack> = HashMap::new();
    for pack in current {
        let key = format!("{}:{}", pack.layer.as_str(), pack.slug);
        let change = match previous.get(&key) {
            None => Some(LayerChange {
                layer: pack.layer.as_str().to_string(),
                stamp: pack.stamp(),
                text: pack.prompt_text(),
                domain: None,
            }),
            Some(prev) if prev.content_hash != pack.content_hash => {
                let diff = pack.diff_text(prev);
                (!diff.trim().is_empty()).then(|| LayerChange {
                    layer: pack.layer.as_str().to_string(),
                    stamp: pack.stamp(),
                    text: diff,
                    domain: None,
                })
            }
            Some(_) => None,
        };
        if let Some(change) = change {
            spawn_layers_changed(state.clone(), change);
        }
        next.insert(key, pack);
    }
    apply_pack_floors(state, &next);
    for (key, gone) in previous.iter() {
        if !next.contains_key(key) {
            spawn_layers_changed(
                state.clone(),
                LayerChange {
                    layer: gone.layer.as_str().to_string(),
                    stamp: format!("{}:removed", gone.stamp()),
                    text: format!(
                        "The {} layer `{}` was removed. Drop what came only from it.",
                        gone.layer.as_str(),
                        gone.slug
                    ),
                    domain: None,
                },
            );
        }
    }
    *previous = next;
}

/// The two things a seat never decides for itself (PRD 6.8): a pack's laws
/// become locked Blocked entries in every seat's operation policy, and a
/// pack's standards and question defaults become the seat's input values
/// where it declares the same semantic id and has no value yet. Company
/// packs win over franchise over industry.
pub fn apply_pack_floors(state: &AppState, packs: &HashMap<String, napp::Pack>) {
    let mut ordered: Vec<&napp::Pack> = packs.values().collect();
    ordered.sort_by_key(|p| p.layer.rank());
    let mut laws: Vec<(String, String)> = Vec::new();
    let mut values: HashMap<String, serde_json::Value> = HashMap::new();
    for pack in &ordered {
        laws.extend(pack.ceilings());
        for (id, v) in pack.defaults() {
            values.insert(id, v); // higher layers come later and overwrite
        }
    }
    let seats = state.store.list_agents(10_000, 0).unwrap_or_default();
    for seat in seats.iter().filter(|a| a.is_app.unwrap_or(0) == 0) {
        // Laws → locked Blocked in the one policy.
        if !laws.is_empty() {
            let current = crate::entity_config::resolve_for_chat(&state.store, "agent", &seat.id)
                .and_then(|c| c.operation_policy);
            let mut policy = tools::policy::OperationPolicy::from_json(current.as_deref());
            let mut changed = false;
            for (op, law) in &laws {
                let suffix = tools::plugin_tool::port_suffix(op);
                let already = policy.operations.get(&suffix).is_some_and(|r| r.is_law());
                if already {
                    continue;
                }
                policy.operations.insert(
                    suffix,
                    tools::policy::OperationRule {
                        access: tools::policy::OperationAccess::Blocked,
                        bounds: None,
                        source: Some(format!("law:{law}")),
                        evidence: None,
                        granted_at: Some(chrono::Utc::now().timestamp()),
                        locked: true,
                    },
                );
                changed = true;
            }
            if changed {
                let patch = serde_json::json!({ "operationPolicy": policy.to_json() });
                if let Err(e) = state.store.upsert_entity_config("agent", &seat.id, &patch) {
                    warn!(agent = %seat.id, error = %e, "pack laws: policy write failed");
                }
            }
        }
        // Standards and defaults → the seat's inputs, by semantic id, only
        // where the seat asks the question and has no value.
        if !values.is_empty() {
            let fm: serde_json::Value = serde_json::from_str(&seat.frontmatter).unwrap_or_default();
            let mut vals: serde_json::Value =
                serde_json::from_str(&seat.input_values).unwrap_or_else(|_| serde_json::json!({}));
            let mut changed = false;
            if let Some(inputs) = fm.get("inputs").and_then(|v| v.as_array()) {
                for input in inputs {
                    let Some(id) = input.get("id").and_then(|v| v.as_str()) else { continue };
                    let key = input
                        .get("key")
                        .and_then(|v| v.as_str())
                        .or_else(|| input.get("name").and_then(|v| v.as_str()))
                        .unwrap_or(id);
                    let money = input.get("money").and_then(|v| v.as_bool()).unwrap_or(false);
                    if money {
                        continue; // never defaulted
                    }
                    let has = vals.get(key).is_some_and(|v| !v.is_null() && v != "");
                    if has {
                        continue;
                    }
                    if let Some(v) = values.get(id) {
                        vals[key] = v.clone();
                        changed = true;
                    }
                }
            }
            if changed {
                if let Err(e) = state.store.update_agent_input_values(&seat.id, &vals.to_string()) {
                    warn!(agent = %seat.id, error = %e, "pack defaults: input write failed");
                }
            }
        }
    }
}

/// At boot: seats that have never written a section while packs exist read
/// every pack now. Seats with a section are left alone; the watcher handles
/// later changes.
pub fn first_read_for_unstamped_seats(state: &AppState, packs: &HashMap<String, napp::Pack>) {
    if packs.is_empty() {
        return;
    }
    apply_pack_floors(state, packs);
    let unstamped: Vec<db::models::Agent> = state
        .store
        .list_agents(10_000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_enabled == 1 && a.is_app.unwrap_or(0) == 0 && a.context_stamp.is_none())
        .collect();
    if unstamped.is_empty() {
        return;
    }
    let mut ordered: Vec<&napp::Pack> = packs.values().collect();
    ordered.sort_by_key(|p| p.layer.rank());
    let text = ordered
        .iter()
        .map(|p| format!("# {} layer: {}\n\n{}", p.layer.as_str(), p.slug, p.prompt_text()))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    let stamp = ordered.iter().map(|p| p.stamp()).collect::<Vec<_>>().join(",");
    let change = LayerChange { layer: "all".to_string(), stamp, text, domain: None };
    let state = state.clone();
    tokio::spawn(async move {
        let limit = Arc::new(tokio::sync::Semaphore::new(PARALLEL_SEATS));
        let change = Arc::new(change);
        let mut handles = Vec::new();
        for seat in unstamped {
            let permit = limit.clone().acquire_owned().await.expect("semaphore");
            let state = state.clone();
            let change = change.clone();
            handles.push(tokio::spawn(async move {
                run_for_seat(&state, &seat, &change).await;
                drop(permit);
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::subscribes;

    #[test]
    fn a_subscription_covers_its_domain_and_children_only() {
        let fm = r#"{"subscribes": ["finance.ar.*", "parties.carriers"]}"#;
        assert!(subscribes(fm, "finance.ar"));
        assert!(subscribes(fm, "finance.ar.dunning"));
        assert!(!subscribes(fm, "finance.ap"));
        assert!(subscribes(fm, "parties.carriers"));
        assert!(!subscribes(fm, "parties.carriers.x"));
        assert!(!subscribes("{}", "finance.ar"));
        assert!(subscribes(r#"{"subscribes": ["*"]}"#, "anything"));
    }
}
