//! The update run: each seat reads a layer change once and writes its own
//! context section (Playbook PRD v0.7 §6.8, implementation PRD R15).
//!
//! A seat meets its industry, franchise, and company layers the way a new
//! hire meets the handbook: it reads it once, decides what matters to its
//! job, and works from what it wrote down. Nothing about the layers is
//! fetched at work time. This module is the trigger and the run; the seat's
//! `context` tool is the write; `build_static` is the read.
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
                info!(agent = %seat.name, against = %change.stamp, "context section written");
            } else {
                warn!(agent = %seat.name, against = %change.stamp, "update run ended without a context write; previous section kept, marked stale");
            }
        }
        _ => {}
    }
}

fn update_prompt(seat: &db::models::Agent, change: &LayerChange) -> String {
    let current = seat
        .context_section
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(none yet)");
    format!(
        "[Update run — not an owner message]\n\
A layer your company works by has changed: {layer} ({stamp}).\n\n\
Read it below. Decide what in it matters to YOUR job: your seat, your workflows, the operations you perform, the parties you deal with, the words this company uses. Then call the `context` tool with action \"write\" and the WHOLE section you will work by from now on: the facts and rules that matter to you, in your own words, in markdown, as short as it can be and no shorter. Keep what still holds from your current section, drop what this change retires, and say what changed.\n\n\
Two things are not yours to decide and must not appear as rules in your section: which operations you may perform (laws and your ceiling set that), and the exact values of answered questions (those are in your configured inputs). Do not do any other work in this run. Do not ask questions; if something is unclear, say so inside the section.\n\n\
## Your current section\n\n{current}\n\n\
## The layer\n\n{text}\n",
        layer = change.layer,
        stamp = change.stamp,
        current = current,
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

/// At boot: seats that have never written a section while packs exist read
/// every pack now. Seats with a section are left alone; the watcher handles
/// later changes.
pub fn first_read_for_unstamped_seats(state: &AppState, packs: &HashMap<String, napp::Pack>) {
    if packs.is_empty() {
        return;
    }
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
