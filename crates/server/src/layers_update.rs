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

/// The one event name for a layer change, on the company bus (where a seat's
/// workflow binds it) and on the client rail (where the owner's layers screen
/// re-reads). Both say the same word so there is one thing to listen for.
pub const LAYERS_CHANGED_EVENT: &str = "layers_changed";

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
        LAYERS_CHANGED_EVENT,
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
        user_id: String::new(),
        channel: "layers".to_string(),
        origin: tools::Origin::System,
        door: types::permissions::Door::Chat,
        agent_id: seat.id.clone(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::COMM.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        attachments: vec![],
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
        coworker: None,
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

/// One pack whose files on disk differ from what the seats have read, parked
/// until the owner applies it.
///
/// The owner edits a layer the way they edit any document: a few saves, a
/// reread, another save. A save is not a decision to teach forty-eight seats —
/// so each save leaves exactly one entry per pack here, replaced rather than
/// appended, and the owner sees the current state of their editing instead of a
/// history of keystrokes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingLayer {
    /// `industry` | `franchise` | `company`.
    pub layer: String,
    pub slug: String,
    /// The pack's display name.
    pub name: String,
    /// `added` | `changed` | `removed`.
    pub kind: String,
    pub stamp: String,
    /// The stamp the seats' sections are written against today.
    pub previous_stamp: Option<String>,
    /// What a seat will read on apply: the whole pack when added, the unified
    /// diff when changed, the retirement note when removed.
    pub diff: String,
    pub detected_at: i64,
    /// The pack as it was scanned. Applying needs no rescan and a restart
    /// resumes exactly what was detected. Never in an API response.
    #[serde(default)]
    pub pack: Option<napp::Pack>,
}

impl PendingLayer {
    fn change(&self) -> LayerChange {
        LayerChange {
            layer: self.layer.clone(),
            stamp: self.stamp.clone(),
            text: self.diff.clone(),
            domain: None,
        }
    }
}

/// Where the applied snapshot and the parked edits live. Deliberately not under
/// `packs/`: a file written there would trip the very watcher that is watching
/// for the owner's edits.
fn store_path(name: &str) -> Option<std::path::PathBuf> {
    config::data_dir().ok().map(|d| d.join(name))
}

const APPLIED_FILE: &str = "layers-applied.json";
const PENDING_FILE: &str = "layers-pending.json";

/// The packs as the seats last read them. `None` when this Nebo has never
/// applied anything — the caller seeds it from the current scan, which is what
/// an install that predates parking looks like: nothing pending, no runs.
pub fn load_applied() -> Option<HashMap<String, napp::Pack>> {
    let path = store_path(APPLIED_FILE)?;
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!(error = %e, path = %path.display(), "layers: applied snapshot unreadable; reseeding from disk");
            None
        }
    }
}

pub fn save_applied(packs: &HashMap<String, napp::Pack>) {
    let Some(path) = store_path(APPLIED_FILE) else { return };
    match serde_json::to_string(packs) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                warn!(error = %e, path = %path.display(), "layers: applied snapshot not written");
            }
        }
        Err(e) => warn!(error = %e, "layers: applied snapshot not serializable"),
    }
}

/// The parked edits from the last run. A restart must not lose what the owner
/// was in the middle of, and must not silently apply it either.
pub fn load_pending() -> Vec<PendingLayer> {
    let Some(path) = store_path(PENDING_FILE) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        warn!(error = %e, path = %path.display(), "layers: parked edits unreadable; starting empty");
        Vec::new()
    })
}

fn save_pending(list: &[PendingLayer]) {
    let Some(path) = store_path(PENDING_FILE) else { return };
    match serde_json::to_string(list) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                warn!(error = %e, path = %path.display(), "layers: parked edits not written");
            }
        }
        Err(e) => warn!(error = %e, "layers: parked edits not serializable"),
    }
}

/// One entry per pack: an edit to a pack that is already parked brings its entry
/// up to date rather than adding a second one.
fn park(pending: &mut Vec<PendingLayer>, mut entry: PendingLayer, now: i64) {
    match pending
        .iter_mut()
        .find(|p| p.layer == entry.layer && p.slug == entry.slug)
    {
        Some(existing) => {
            entry.detected_at = if existing.diff == entry.diff { existing.detected_at } else { now };
            *existing = entry;
        }
        None => {
            entry.detected_at = now;
            pending.push(entry);
        }
    }
}

fn drop_parked(pending: &mut Vec<PendingLayer>, layer: &str, slug: &str) {
    pending.retain(|p| p.layer != layer || p.slug != slug);
}

/// Detection. Compares the scan against the applied snapshot and leaves one
/// pending entry per pack that differs. It raises nothing: no seat run, no law,
/// no standard, no company policy moves here.
///
/// `applied` is touched in exactly one case — when the scan matches it again,
/// because the owner undid their edit or moved only a file no seat reads. Then
/// the pack's entry is refreshed and its parked edit dropped, which is the truth:
/// there is nothing to apply.
pub fn record_pending(
    applied: &mut HashMap<String, napp::Pack>,
    pending: &mut Vec<PendingLayer>,
    current: Vec<napp::Pack>,
) -> Detected {
    let now = chrono::Utc::now().timestamp();
    let before = fingerprint(pending);
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut snapshot_moved = false;
    for pack in current {
        let key = format!("{}:{}", pack.layer.as_str(), pack.slug);
        seen.insert(key.clone());
        let layer = pack.layer.as_str().to_string();
        let entry = match applied.get(&key) {
            None => PendingLayer {
                layer,
                slug: pack.slug.clone(),
                name: pack.name.clone(),
                kind: "added".to_string(),
                stamp: pack.stamp(),
                previous_stamp: None,
                diff: pack.prompt_text(),
                detected_at: now,
                pack: Some(pack),
            },
            Some(prev) if prev.content_hash != pack.content_hash => {
                let diff = pack.diff_text(prev);
                if diff.trim().is_empty() {
                    // Nothing a seat would read changed: the pack's own bytes
                    // moved (a picture in `reference/`, a trailing newline) but
                    // its text and its frontmatter did not.
                    drop_parked(pending, &layer, &pack.slug);
                    applied.insert(key, pack);
                    snapshot_moved = true;
                    continue;
                }
                PendingLayer {
                    layer,
                    slug: pack.slug.clone(),
                    name: pack.name.clone(),
                    kind: "changed".to_string(),
                    stamp: pack.stamp(),
                    previous_stamp: Some(prev.stamp()),
                    diff,
                    detected_at: now,
                    pack: Some(pack),
                }
            }
            // Byte-identical to what the seats read: the owner reverted.
            Some(_) => {
                drop_parked(pending, &layer, &pack.slug);
                continue;
            }
        };
        park(pending, entry, now);
    }
    for (key, gone) in applied.iter() {
        if seen.contains(key) {
            continue;
        }
        park(
            pending,
            PendingLayer {
                layer: gone.layer.as_str().to_string(),
                slug: gone.slug.clone(),
                name: gone.name.clone(),
                kind: "removed".to_string(),
                stamp: format!("{}:removed", gone.stamp()),
                previous_stamp: Some(gone.stamp()),
                diff: format!(
                    "The {} layer `{}` was removed. Drop what came only from it.",
                    gone.layer.as_str(),
                    gone.slug
                ),
                detected_at: now,
                pack: None,
            },
            now,
        );
    }
    Detected { pending_changed: fingerprint(pending) != before, snapshot_moved }
}

/// What one detection pass found.
pub struct Detected {
    /// The parked list is not what it was: the owner's screen has something new
    /// to read, and the client rail is told so it does not wait for a reload.
    pub pending_changed: bool,
    /// The applied snapshot moved on its own — an edit was undone, or bytes no
    /// seat reads changed — so it needs writing back.
    pub snapshot_moved: bool,
}

/// Enough of the parked list to tell whether it moved.
fn fingerprint(pending: &[PendingLayer]) -> Vec<(String, String, i64, usize)> {
    pending
        .iter()
        .map(|p| (p.slug.clone(), p.stamp.clone(), p.detected_at, p.diff.len()))
        .collect()
}

/// Application. Lifts the entries to apply out of the parked list and moves
/// their packs into the applied snapshot. The caller reads the floors off the new
/// snapshot and raises the changes; nothing here reaches a seat by itself.
pub fn take_for_apply(
    applied: &mut HashMap<String, napp::Pack>,
    pending: &mut Vec<PendingLayer>,
    slugs: Option<&[String]>,
) -> Vec<PendingLayer> {
    let wanted = |p: &PendingLayer| match slugs {
        None => true,
        Some(list) => list.iter().any(|w| w == &p.slug),
    };
    let taken: Vec<PendingLayer> = pending.iter().filter(|p| wanted(p)).cloned().collect();
    pending.retain(|p| !wanted(p));
    for entry in &taken {
        let key = format!("{}:{}", entry.layer, entry.slug);
        match &entry.pack {
            Some(pack) => applied.insert(key, pack.clone()),
            None => applied.remove(&key),
        };
    }
    taken
}

/// The watcher's entry: park what changed and tell nobody. A company of
/// forty-eight seats and three saves while the owner is editing used to be a
/// hundred and forty-four hidden runs for one edit; now it is three parked
/// entries collapsed into one, and the learning happens when the owner says so.
///
/// It reads the directory ITSELF, inside the locks that record the reading.
/// A scan taken before the lock describes a directory that may have moved on
/// while the lock was waited for, and two rescans racing could then record
/// them out of order: the loser parked a pack the owner had just deleted, and
/// nothing un-parked it, because the disk had stopped changing and no further
/// rescan was coming. Applying that phantom entry put a pack back into the
/// applied snapshot with no pack on disk — a layer the owner could not remove
/// until they touched the folder again.
pub async fn detect_changes(state: &AppState, packs_dir: &std::path::Path) -> usize {
    let (count, changed, slugs) = {
        let mut applied = state.packs.write().await;
        let mut pending = state.pending_layers.write().await;
        let found = record_pending(&mut applied, &mut pending, napp::scan_packs(packs_dir));
        if found.snapshot_moved {
            save_applied(&applied);
        }
        if found.pending_changed {
            save_pending(&pending);
            info!(
                pending = pending.len(),
                "layers: edits parked; no seat, law or company policy moves until the owner applies"
            );
        }
        let slugs: Vec<String> = pending.iter().map(|p| p.slug.clone()).collect();
        (pending.len(), found.pending_changed, slugs)
    };
    // The owner's screen is told here rather than by each caller, so a pack
    // dropped into the folder or edited outside the app reaches it too — not only
    // the saves that happen to come back through a handler.
    if changed {
        state.hub.broadcast(
            LAYERS_CHANGED_EVENT,
            serde_json::json!({ "pending": slugs, "applied": Vec::<String>::new() }),
        );
    }
    count
}

/// The owner applies. Every parked edit lands as one coherent moment: the floors
/// come off the new snapshot first, because a seat's grant is checked against the
/// company's policy while the seat is writing, and then each applied pack raises
/// `layers_changed` and the seats that read it get their one run.
///
/// The outward half happens here too, and only here: the same event that makes
/// every seat re-read its layers re-renders the company's rules for an outside
/// coding tool (`agents_export`). One trigger, no schedule.
///
/// Returns the slugs applied and how many seats got a run.
pub async fn apply_pending(state: &AppState, slugs: Option<Vec<String>>) -> (Vec<String>, usize) {
    let taken = {
        let mut applied = state.packs.write().await;
        let mut pending = state.pending_layers.write().await;
        let taken = take_for_apply(&mut applied, &mut pending, slugs.as_deref());
        if taken.is_empty() {
            return (Vec::new(), 0);
        }
        save_applied(&applied);
        save_pending(&pending);
        apply_pack_floors(state, &applied);
        crate::agents_export::rerender(state, &applied);
        taken
    };
    let mut seats = 0usize;
    let mut done = Vec::with_capacity(taken.len());
    for entry in taken {
        let change = entry.change();
        seats = seats.max(seats_for(state, &change).len());
        info!(layer = %entry.layer, slug = %entry.slug, kind = %entry.kind, "layers: applied");
        done.push(entry.slug);
        spawn_layers_changed(state.clone(), change);
    }
    let still_parked: Vec<String> =
        state.pending_layers.read().await.iter().map(|p| p.slug.clone()).collect();
    state.hub.broadcast(
        LAYERS_CHANGED_EVENT,
        serde_json::json!({ "pending": still_parked, "applied": done, "seats": seats }),
    );
    (done, seats)
}

/// Detect and apply in one step, for the one act that is already explicit:
/// installing an org or a pack from the marketplace. The owner asked for it by
/// name, so the install is the apply and it does not park. Editing a layer file
/// parks instead — that is the whole point of the split.
pub async fn detect_and_apply(
    state: &AppState,
    packs_dir: &std::path::Path,
    slugs: Option<Vec<String>>,
) -> (Vec<String>, usize) {
    detect_changes(state, packs_dir).await;
    apply_pending(state, slugs).await
}

/// The two things a seat never decides for itself (PRD 6.8): a pack's laws
/// become locked Blocked entries in every seat's operation policy, and a
/// pack's standards and question defaults become the seat's input values
/// where it declares the same semantic id and has no value yet. Company
/// packs win over franchise over industry.
/// The six standards the runtime itself reads, and the only ids reserved to
/// it. A company names everything else in its own namespace; these six are
/// the same in every company or Nebo could not read a stranger's company at
/// all. They are the company's unattended bounds: what the whole workforce
/// may spend in a day, per counterparty, in one operation, how many
/// irreversible operations a day, how fresh a money grant must be, and when
/// the owner is paged.
mod company_ids {
    pub const PER_DAY_CENTS: &str = "company.unattended.spend_per_day_cents";
    pub const PER_COUNTERPARTY_DAY_CENTS: &str =
        "company.unattended.spend_per_counterparty_day_cents";
    pub const MAX_AMOUNT_CENTS: &str = "company.unattended.spend_per_operation_cents";
    pub const IRREVERSIBLE_PER_DAY: &str = "company.unattended.irreversible_per_day";
    pub const FRESHNESS_SECS: &str = "company.unattended.grant_freshness_secs";
    pub const PAGES: &str = "company.owner.pages";
}

/// The company level of the one policy, read from the company layer itself.
///
/// There is no artifact above the company: the owner's purpose is
/// `COMPANY.md`'s own, the unattended bounds are standards under the six
/// reserved ids, and the operations reserved to the owner's own hand are the
/// company's laws marked `reserved_to: owner`. A franchise or industry layer
/// cannot set any of this; only the company pack is read here.
fn company_policy_from(packs: &HashMap<String, napp::Pack>) -> Option<tools::policy::CompanyPolicy> {
    let pack = packs
        .values()
        .find(|p| matches!(p.layer, napp::pack::PackLayer::Company))?;
    let values = pack.defaults();
    let num = |id: &str| -> Option<i64> {
        values.get(id).and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.replace([',', '$', '_'], "").trim().parse().ok(),
            _ => None,
        })
    };
    let purpose = pack
        .frontmatter
        .get("purpose")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_matches('"').to_string())
        .or_else(|| {
            pack.body
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .map(String::from)
        })
        .unwrap_or_default();
    Some(tools::policy::CompanyPolicy {
        purpose,
        reserved: pack
            .reserved_ops()
            .iter()
            .map(|op| tools::plugin_tool::port_suffix(op))
            .collect(),
        daily: tools::policy::Bounds {
            max_amount_cents: num(company_ids::MAX_AMOUNT_CENTS),
            per_day_cents: num(company_ids::PER_DAY_CENTS),
            per_day_count: num(company_ids::IRREVERSIBLE_PER_DAY),
            per_counterparty_day_cents: num(company_ids::PER_COUNTERPARTY_DAY_CENTS),
            counterparty_class: None,
            freshness_secs: num(company_ids::FRESHNESS_SECS),
        },
        pages: values.get(company_ids::PAGES).cloned(),
    })
}

/// One law on one seat: a locked rule on the operation, written by the pack.
fn write_law(state: &AppState, seat_id: &str, op: &str, effect: types::permissions::Effect, pack: &str) {
    use types::permissions::{Rule, RuleKey, RuleSource, Scope, Writer};
    let suffix = tools::plugin_tool::port_suffix(op);
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(seat_id.to_string()),
        key: RuleKey::Operation(suffix),
        field: None,
        effect,
        money: None,
        source: RuleSource::Law { pack: pack.to_string() },
        locked: true,
        created_at: chrono::Utc::now().timestamp(),
    };
    if let Err(e) = state.store.write_permission_rule(&rule, &Writer::Package { package: pack.to_string() }) {
        warn!(agent = %seat_id, error = %e, "pack laws: rule write failed");
    }
}

pub fn apply_pack_floors(state: &AppState, packs: &HashMap<String, napp::Pack>) {
    // The company layer's own policy, before the seats: a seat's grant is
    // checked against it, so it must be current when the seats are written.
    if let Some(policy) = company_policy_from(packs) {
        let current = tools::policy::CompanyPolicy::from_json(
            state.store.get_company_policy().ok().flatten().as_deref(),
        );
        if current != policy {
            match state.store.set_company_policy(&policy.to_json()) {
                Ok(_) => info!(
                    reserved = policy.reserved.len(),
                    "company layer: policy read from COMPANY.md, its standards and its laws"
                ),
                Err(e) => warn!(error = %e, "company layer: policy write failed"),
            }
        }
    }
    let mut ordered: Vec<&napp::Pack> = packs.values().collect();
    ordered.sort_by_key(|p| p.layer.rank());
    let mut laws: Vec<(String, String)> = Vec::new();
    let reserved: Vec<String> = company_policy_from(packs).map(|c| c.reserved).unwrap_or_default();
    let mut values: HashMap<String, serde_json::Value> = HashMap::new();
    for pack in &ordered {
        laws.extend(pack.ceilings());
        for (id, v) in pack.defaults() {
            values.insert(id, v); // higher layers come later and overwrite
        }
    }
    let seats = state.store.list_agents(10_000, 0).unwrap_or_default();
    for seat in seats.iter().filter(|a| a.is_app.unwrap_or(0) == 0) {
        // Laws → locked deny rules on the seat, and the operations the
        // company reserves to the owner → locked ask rules: an employee
        // rule on the same operation never outranks them.
        for (op, law) in &laws {
            write_law(state, &seat.id, op, types::permissions::Effect::Deny, law);
        }
        for op in &reserved {
            write_law(state, &seat.id, op, types::permissions::Effect::Ask, "company");
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
    use super::{company_policy_from, record_pending, subscribes, take_for_apply, PendingLayer};
    use std::collections::HashMap;

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

    /// The company layer's own policy: the money bounds come from the six
    /// reserved standard ids, the owner's hand comes from a law marked
    /// `reserved_to: owner`, and the purpose comes from COMPANY.md itself.
    /// If this drifts, a company's spending ceiling silently becomes no
    /// ceiling, so it is checked rather than trusted.
    #[test]
    fn the_company_layer_is_the_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme");
        let w = |rel: &str, body: &str| {
            let f = dir.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        w(
            "COMPANY.md",
            "---\ntype: company\ncompany: Acme\nversion: 1.0.0\npurpose: \"Fix roofs and get paid.\"\n---\n\nFix roofs and get paid.\n",
        );
        w("standards/day.md", "---\nid: company.unattended.spend_per_day_cents\nvalue: 1000000\n---\n\nA day.\n");
        w("standards/op.md", "---\nid: company.unattended.spend_per_operation_cents\nvalue: 250000\n---\n\nOne.\n");
        w("standards/cp.md", "---\nid: company.unattended.spend_per_counterparty_day_cents\nvalue: 500000\n---\n\nOne party.\n");
        w("standards/irr.md", "---\nid: company.unattended.irreversible_per_day\nvalue: 20\n---\n\nCount.\n");
        w("standards/fresh.md", "---\nid: company.unattended.grant_freshness_secs\nvalue: 86400\n---\n\nFresh.\n");
        w("standards/own.md", "---\nid: acme.crew_size\nvalue: 6\n---\n\nOurs, not the runtime's.\n");
        w("laws/equity.md", "---\nlaw: Equity\nceiling: [\"equity.issue\"]\nreserved_to: owner\n---\n\nThe owner's.\n");
        w("laws/pay.md", "---\nlaw: Payments\nceiling: [\"ledger.payment.send\"]\n---\n\nNo seat pays unattended.\n");

        let pack = napp::pack::load_pack(&dir).unwrap();
        let mut packs = HashMap::new();
        packs.insert("company:acme".to_string(), pack);
        let policy = company_policy_from(&packs).expect("the company layer is the policy");

        assert_eq!(policy.purpose, "Fix roofs and get paid.");
        assert_eq!(policy.daily.per_day_cents, Some(1_000_000));
        assert_eq!(policy.daily.max_amount_cents, Some(250_000));
        assert_eq!(policy.daily.per_counterparty_day_cents, Some(500_000));
        assert_eq!(policy.daily.per_day_count, Some(20));
        assert_eq!(policy.daily.freshness_secs, Some(86_400));
        // The owner's hand is reserved. The blocked law is not: it reaches
        // every seat as a law, which is a different mechanism.
        assert_eq!(policy.reserved, vec!["equity.issue".to_string()]);
        assert!(!policy.is_reserved("ledger.payment.send"));
    }

    /// The owner edits a layer the way they edit any document: a save, a reread,
    /// another save. Three saves to one pack must leave ONE parked entry — the
    /// current state of their editing, not a history of keystrokes — and not a
    /// single seat run. A company of forty-eight seats and three saves used to be
    /// a hundred and forty-four hidden runs for one edit.
    ///
    /// Nor may anything else move early: the laws, the standards and the
    /// company's own policy all stay where the seats last read them until the
    /// owner applies, so the whole edit lands as one coherent moment.
    #[test]
    fn saves_accumulate_into_one_entry_and_nothing_moves_until_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme");
        let w = |rel: &str, body: &str| {
            let f = dir.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        let day = |cents: &str| {
            format!("---\nid: company.unattended.spend_per_day_cents\nvalue: {cents}\n---\n\nA day.\n")
        };
        w("COMPANY.md", "---\ntype: company\ncompany: Acme\nversion: 1.0.0\npurpose: \"Fix roofs and get paid.\"\n---\n\nFix roofs and get paid.\n");
        w("standards/day.md", &day("1000000"));
        w("laws/pay.md", "---\nlaw: Payments\nceiling: [\"ledger.payment.send\"]\n---\n\nNo seat pays unattended.\n");

        // What the seats have actually read.
        let mut applied: HashMap<String, napp::Pack> = HashMap::new();
        applied.insert("company:acme".to_string(), napp::pack::load_pack(&dir).unwrap());
        let mut pending: Vec<PendingLayer> = Vec::new();

        // Save one: a bigger daily ceiling.
        w("standards/day.md", &day("1500000"));
        record_pending(&mut applied, &mut pending, vec![napp::pack::load_pack(&dir).unwrap()]);
        assert_eq!(pending.len(), 1, "one pack, one entry");

        // Save two: the owner rewords the law while they are in there.
        w("laws/pay.md", "---\nlaw: Payments\nceiling: [\"ledger.payment.send\"]\n---\n\nNo seat pays unattended, ever.\n");
        record_pending(&mut applied, &mut pending, vec![napp::pack::load_pack(&dir).unwrap()]);
        assert_eq!(pending.len(), 1, "the same pack replaces its entry: {pending:?}");

        // Save three: they settle on a number.
        w("standards/day.md", &day("2500000"));
        record_pending(&mut applied, &mut pending, vec![napp::pack::load_pack(&dir).unwrap()]);
        assert_eq!(pending.len(), 1, "still one entry after three saves");

        let parked = &pending[0];
        assert_eq!(parked.kind, "changed");
        assert_eq!(parked.slug, "acme");
        assert_eq!(parked.previous_stamp.as_deref(), Some("company:acme@1.0.0"));
        // The entry carries the CURRENT state of the editing, both files, as a
        // unified diff and not a reprint.
        assert!(parked.diff.contains("--- a/standards/day.md"), "{}", parked.diff);
        assert!(parked.diff.contains("+value: 2500000"), "{}", parked.diff);
        assert!(!parked.diff.contains("1500000"), "the abandoned number is gone: {}", parked.diff);
        assert!(parked.diff.contains("--- a/laws/pay.md"), "{}", parked.diff);

        // Nothing has moved. The snapshot the seats read is still the old one,
        // and so is the company's own spending ceiling.
        assert_eq!(
            company_policy_from(&applied).unwrap().daily.per_day_cents,
            Some(1_000_000),
            "the company policy must not move before the owner applies"
        );

        // The owner applies. Now it moves, all at once.
        let taken = take_for_apply(&mut applied, &mut pending, None);
        assert_eq!(taken.len(), 1);
        assert!(pending.is_empty(), "applying clears what it applied");
        assert_eq!(
            company_policy_from(&applied).unwrap().daily.per_day_cents,
            Some(2_500_000)
        );
        assert_eq!(applied["company:acme"].stamp(), "company:acme@1.0.0");

        // Applying twice raises nothing the second time.
        assert!(take_for_apply(&mut applied, &mut pending, None).is_empty());
    }

    /// An edit the owner undid is not an edit. The pack is byte-identical to what
    /// the seats read again, so the parked entry goes away rather than sitting
    /// there waiting to teach forty-eight seats nothing.
    #[test]
    fn an_undone_edit_unparks_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("roofing");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("INDUSTRY.md");
        let original = "---\ntype: playbook\nindustry: roofing\nversion: 0.1.0\n---\n\n# Roofing\n\nHow the trade runs.\n";
        std::fs::write(&marker, original).unwrap();

        let mut applied: HashMap<String, napp::Pack> = HashMap::new();
        applied.insert("industry:roofing".to_string(), napp::pack::load_pack(&dir).unwrap());
        let mut pending: Vec<PendingLayer> = Vec::new();

        std::fs::write(&marker, original.replace("How the trade runs.", "How the trade really runs.")).unwrap();
        record_pending(&mut applied, &mut pending, vec![napp::pack::load_pack(&dir).unwrap()]);
        assert_eq!(pending.len(), 1);

        std::fs::write(&marker, original).unwrap();
        record_pending(&mut applied, &mut pending, vec![napp::pack::load_pack(&dir).unwrap()]);
        assert!(pending.is_empty(), "nothing to apply: {pending:?}");
    }

    /// The owner applies one pack and leaves the other parked. Only the named one
    /// moves into the snapshot the seats read.
    #[test]
    fn applying_by_slug_leaves_the_other_pack_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let write_pack = |slug: &str, marker: &str, body: &str| {
            let d = tmp.path().join(slug);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(marker), body).unwrap();
            d
        };
        let ind = write_pack("roofing", "INDUSTRY.md", "---\nindustry: roofing\nversion: 0.1.0\n---\n\n# Roofing\n\nOne.\n");
        let co = write_pack("acme", "COMPANY.md", "---\ncompany: Acme\nversion: 1.0.0\n---\n\n# Acme\n\nTwo.\n");

        // Neither has ever been applied: both are new.
        let mut applied: HashMap<String, napp::Pack> = HashMap::new();
        let mut pending: Vec<PendingLayer> = Vec::new();
        let scan = vec![napp::pack::load_pack(&ind).unwrap(), napp::pack::load_pack(&co).unwrap()];
        record_pending(&mut applied, &mut pending, scan);
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().all(|p| p.kind == "added"));
        // A new pack is read whole, not as a diff against nothing.
        let first = pending.iter().find(|p| p.slug == "roofing").unwrap();
        assert!(first.diff.contains("(industry layer, version 0.1.0)"), "{}", first.diff);
        assert!(first.diff.contains("# Roofing"), "{}", first.diff);

        let applied_slugs = take_for_apply(&mut applied, &mut pending, Some(&["acme".to_string()]));
        assert_eq!(applied_slugs.len(), 1);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].slug, "roofing");
        assert!(applied.contains_key("company:acme"));
        assert!(!applied.contains_key("industry:roofing"), "the parked pack stays out");
    }

    /// A pack the owner deleted is parked as a retirement, and applying it takes
    /// the pack out of the snapshot rather than leaving a ghost behind.
    #[test]
    fn a_removed_pack_parks_as_a_retirement() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("roofing");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("INDUSTRY.md"), "---\nindustry: roofing\nversion: 0.1.0\n---\n\n# Roofing\n").unwrap();
        let mut applied: HashMap<String, napp::Pack> = HashMap::new();
        applied.insert("industry:roofing".to_string(), napp::pack::load_pack(&dir).unwrap());
        let mut pending: Vec<PendingLayer> = Vec::new();

        record_pending(&mut applied, &mut pending, Vec::new());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, "removed");
        assert_eq!(pending[0].stamp, "industry:roofing@0.1.0:removed");
        assert!(pending[0].diff.contains("Drop what came only from it"));
        assert!(applied.contains_key("industry:roofing"), "not gone until applied");

        take_for_apply(&mut applied, &mut pending, None);
        assert!(applied.is_empty());
    }
}
