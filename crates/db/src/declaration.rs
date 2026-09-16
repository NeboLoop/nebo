//! Whose declaration wins when a package update lands on an employee the owner
//! has already shaped.
//!
//! The `name_locked` contract, applied to the seat's declaration — with one
//! difference forced by the shape of the data. A name is one value, so one
//! boolean settles it. The declaration is four collections
//! (`requires.interfaces`, `inputs`, `subscribes`, `ceiling`), and locking a
//! whole collection would mean an update could never again deliver a new
//! question or a corrected capability to an employee the owner once touched.
//! That is its own bug. So the grain here is **per entry**: the owner holds the
//! entries they actually authored, and the package keeps every other entry —
//! including brand new ones.
//!
//! The mechanism is a three-way merge. `PACKAGE_BASELINE` records the
//! declaration as the package last delivered it, which is what makes
//! "the owner changed this" distinguishable from "the package changed this":
//!
//! - base  — what the package said last time (the baseline)
//! - ours  — what is on the row now (base plus whatever the owner did)
//! - theirs — what the update is delivering
//!
//! An entry ours-differs-from-base is the owner's and is held. An entry in base
//! and gone from ours was deleted by the owner and stays deleted. Everything
//! else is the package's to change. Nothing is destroyed: the baseline keeps
//! what the package said, so the record still shows the value the owner's edit
//! superseded.
//!
//! With no baseline the merge takes the package's value wholesale, which is the
//! behaviour for every employee the owner has never edited. The baseline is
//! written at the moment the owner first authors a declaration field (see
//! `note_owner_edit`), because that is the moment the two versions diverge and
//! the pre-edit value on the row is still exactly what the package said.

use serde_json::{Map, Value};

/// Reserved frontmatter key holding the declaration as the package last
/// delivered it. Not part of `agent.json`'s published shape — `napp`'s parser
/// ignores unknown keys, so it rides along without meaning anything to it.
pub const PACKAGE_BASELINE: &str = "package_declared";

/// The declaration fields a package update must merge rather than overwrite,
/// as JSON pointers into the frontmatter. Anything not on this list stays fully
/// the package's: the persona, `workflows`, `skills`, `tools`, `scopes`,
/// `defaults`, `pricing`, `memory.topics`. An owner does not author those, so
/// an update has nothing to fight over.
pub const OWNER_AUTHORED_FIELDS: &[&str] = &[
    "/requires/interfaces",
    "/inputs",
    "/subscribes",
    "/ceiling",
];

/// The identity of one entry inside a declaration collection, so the merge can
/// tell "the same entry, changed" from "a different entry".
///
/// A question is identified by its durable semantic id when it has one and its
/// artifact-local key otherwise — an owner who renames a question's label has
/// edited it, not replaced it. A capability or a fact domain is a bare string
/// and is its own identity. Anything else falls back to its exact JSON, which
/// makes it an all-or-nothing entry rather than silently mismatching.
fn entry_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| o.get("key").and_then(Value::as_str).filter(|s| !s.is_empty()))
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string()),
        _ => v.to_string(),
    }
}

fn keyed(items: &[Value]) -> Vec<(String, Value)> {
    items.iter().map(|v| (entry_key(v), v.clone())).collect()
}

fn find<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Three-way merge of one declaration array, in the package's order, with the
/// owner's own entries appended.
fn merge_array(base: &[Value], ours: &[Value], theirs: &[Value]) -> Vec<Value> {
    let (b, o, t) = (keyed(base), keyed(ours), keyed(theirs));

    let owner_changed = |key: &str| -> bool {
        match (find(&b, key), find(&o, key)) {
            // On the row and not in the baseline: the owner added it.
            (None, Some(_)) => true,
            // On both and different: the owner edited it.
            (Some(was), Some(now)) => was != now,
            _ => false,
        }
    };
    // In the baseline and gone from the row: the owner deleted it. An update
    // must not quietly put it back.
    let owner_removed =
        |key: &str| -> bool { find(&b, key).is_some() && find(&o, key).is_none() };

    let mut out: Vec<Value> = Vec::new();
    for (key, theirs_entry) in &t {
        if owner_removed(key) {
            continue;
        }
        if owner_changed(key) {
            if let Some(ours_entry) = find(&o, key) {
                out.push(ours_entry.clone());
                continue;
            }
        }
        out.push(theirs_entry.clone());
    }
    // The owner's own additions, which the package has never heard of.
    for (key, ours_entry) in &o {
        if find(&t, key).is_none() && owner_changed(key) {
            out.push(ours_entry.clone());
        }
    }
    out
}

/// The same merge for a declaration written as an object (`ceiling`), where the
/// entry's identity is its key.
fn merge_object(base: &Map<String, Value>, ours: &Map<String, Value>, theirs: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, theirs_val) in theirs {
        let owner_removed = base.contains_key(key) && !ours.contains_key(key);
        if owner_removed {
            continue;
        }
        let owner_changed = match (base.get(key), ours.get(key)) {
            (None, Some(_)) => true,
            (Some(was), Some(now)) => was != now,
            _ => false,
        };
        match (owner_changed, ours.get(key)) {
            (true, Some(ours_val)) => out.insert(key.clone(), ours_val.clone()),
            _ => out.insert(key.clone(), theirs_val.clone()),
        };
    }
    for (key, ours_val) in ours {
        if theirs.contains_key(key) {
            continue;
        }
        let owner_added = !base.contains_key(key);
        if owner_added {
            out.insert(key.clone(), ours_val.clone());
        }
    }
    out
}

/// Read a pointer, treating a missing or null value as absent.
fn at<'a>(v: &'a Value, pointer: &str) -> Option<&'a Value> {
    v.pointer(pointer).filter(|x| !x.is_null())
}

/// Write a pointer, creating the intermediate objects it needs.
fn put(v: &mut Value, pointer: &str, value: Value) {
    let segments: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let mut cursor = v;
    for seg in &segments[..segments.len() - 1] {
        if !cursor.get(*seg).map(Value::is_object).unwrap_or(false) {
            cursor[*seg] = Value::Object(Map::new());
        }
        cursor = cursor.get_mut(*seg).expect("just created");
    }
    cursor[segments[segments.len() - 1]] = value;
}

/// The declaration fields of a frontmatter, as a baseline record.
fn declaration_of(frontmatter: &Value) -> Value {
    let mut out = Value::Object(Map::new());
    for field in OWNER_AUTHORED_FIELDS {
        if let Some(v) = at(frontmatter, field) {
            put(&mut out, field, v.clone());
        }
    }
    out
}

/// Record the pre-edit declaration as the package's baseline, for the fields the
/// owner is authoring right now — but only where no baseline exists yet.
///
/// Called from the owner's save. At that instant the value on the row is still
/// exactly what the package delivered, so it is the honest baseline; a later
/// save must not move it, or the owner's earlier edits would look like the
/// package's and be given away on the next update.
pub fn note_owner_edit(frontmatter: &mut Value, pre_edit: &Value, fields: &[&str]) {
    if !frontmatter.is_object() {
        return;
    }
    let mut baseline = frontmatter
        .get(PACKAGE_BASELINE)
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut wrote = false;
    for field in fields {
        if at(&baseline, field).is_some() {
            continue; // the package's word is already on record
        }
        // An absent field records as an explicit empty of the right shape, so
        // "the package said nothing here" is distinguishable from "no baseline".
        let was = at(pre_edit, field).cloned().unwrap_or(match *field {
            "/ceiling" => Value::Object(Map::new()),
            _ => Value::Array(Vec::new()),
        });
        put(&mut baseline, field, was);
        wrote = true;
    }
    if wrote {
        frontmatter[PACKAGE_BASELINE] = baseline;
    }
}

/// Merge a package's incoming frontmatter over what is on the row: the owner's
/// own declaration entries are held, every other entry is the package's, and
/// the baseline moves forward to what this update said.
///
/// The ONE routine for it. Both package-delivery paths call it — the marketplace
/// install/update that writes the row directly, and the boot/watcher filesystem
/// sync — so neither can drift into overwriting the owner's work while the other
/// preserves it.
pub fn merge_package_declaration(existing: &str, incoming: &str) -> String {
    let Ok(ours) = serde_json::from_str::<Value>(existing) else {
        return incoming.to_string();
    };
    let Ok(mut theirs) = serde_json::from_str::<Value>(incoming) else {
        return incoming.to_string();
    };
    if !ours.is_object() || !theirs.is_object() {
        return incoming.to_string();
    }

    // Owner-set runtime state the filesystem knows nothing about. The isolation
    // toggle has always ridden here; without it, every restart silently
    // un-isolated employees whose whole point is sealed memory.
    if let Some(iso) = ours.pointer("/memory/context_isolated").and_then(Value::as_bool) {
        put(&mut theirs, "/memory/context_isolated", Value::Bool(iso));
    }

    // Is this a package delivering a declaration, or one of our own documents
    // coming back? A package's `agent.json` never carries a baseline — only a
    // file we wrote does. The owner's edits are mirrored to
    // `{napp_path}/agent.json`, so the watcher hands that file straight back
    // here; treating it as a package delivery would advance the baseline to the
    // owner's own values, and the next real update would then read their edits
    // as the package's and give every one of them away. So: an owner-side
    // document is taken as it is, and the baseline does not move.
    let is_package_delivery = at(&theirs, &format!("/{PACKAGE_BASELINE}")).is_none();
    if !is_package_delivery {
        return theirs.to_string();
    }

    let baseline = ours
        .get(PACKAGE_BASELINE)
        .filter(|v| v.is_object())
        .cloned();

    // No baseline means the owner has never authored a declaration on this
    // employee, so there is nothing of theirs to hold and the update applies
    // whole — exactly today's behaviour.
    if let Some(base) = baseline {
        for field in OWNER_AUTHORED_FIELDS {
            let base_v = at(&base, field);
            let ours_v = at(&ours, field);
            let theirs_v = at(&theirs, field);
            if base_v.is_none() {
                continue; // the owner never touched this field
            }
            let is_object_field = base_v.map(Value::is_object).unwrap_or(false)
                || ours_v.map(Value::is_object).unwrap_or(false)
                || theirs_v.map(Value::is_object).unwrap_or(false);
            if is_object_field {
                let g = |v: Option<&Value>| {
                    v.and_then(Value::as_object).cloned().unwrap_or_default()
                };
                let merged = merge_object(&g(base_v), &g(ours_v), &g(theirs_v));
                put(&mut theirs, field, Value::Object(merged));
            } else {
                let g = |v: Option<&Value>| {
                    v.and_then(Value::as_array).cloned().unwrap_or_default()
                };
                let merged = merge_array(&g(base_v), &g(ours_v), &g(theirs_v));
                put(&mut theirs, field, Value::Array(merged));
            }
        }
    }

    // The baseline moves to what this update said: the next update measures the
    // owner's edits against this package's word, not a stale one.
    theirs[PACKAGE_BASELINE] = declaration_of(&serde_json::from_str::<Value>(incoming).unwrap_or_default());

    theirs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_edits(frontmatter: &str, edit: impl FnOnce(&mut Value)) -> String {
        let pre: Value = serde_json::from_str(frontmatter).unwrap();
        let mut after = pre.clone();
        edit(&mut after);
        note_owner_edit(&mut after, &pre, OWNER_AUTHORED_FIELDS);
        after.to_string()
    }

    /// The failure this exists to stop: install a bookkeeper, give it a
    /// capability, write a question the trade needs — and a routine package
    /// update must not throw either away. Same shape as
    /// `the_owners_department_survives_a_package_sync`.
    #[test]
    fn the_owners_declaration_survives_a_package_update() {
        let shipped = r#"{
            "requires": {"interfaces": ["ledger"], "plugins": ["quickbooks"]},
            "inputs": [{"id": "finance.ap.invoice_mailbox", "key": "mailbox", "label": "Which mailbox?"}],
            "ceiling": {"ledger.payment.apply": "approval"},
            "subscribes": ["finance.ap"]
        }"#;

        // The owner adds a capability and a question of their own.
        let ours = owner_edits(shipped, |fm| {
            fm["requires"]["interfaces"] = serde_json::json!(["ledger", "mail"]);
            fm["inputs"] = serde_json::json!([
                {"id": "finance.ap.invoice_mailbox", "key": "mailbox", "label": "Which mailbox?"},
                {"id": "trade.permit_number", "key": "permit", "label": "What is your permit number?"}
            ]);
        });

        // The package updates. It knows nothing of the owner's additions, and it
        // corrects a question's wording and adds one of its own.
        let update = r#"{
            "requires": {"interfaces": ["ledger"], "plugins": ["quickbooks"]},
            "inputs": [
                {"id": "finance.ap.invoice_mailbox", "key": "mailbox", "label": "Which mailbox do bills arrive in?"},
                {"id": "finance.ap.terms", "key": "terms", "label": "What payment terms do you offer?"}
            ],
            "ceiling": {"ledger.payment.apply": "approval", "ledger.invoice.send": "approval"},
            "subscribes": ["finance.ap", "parties.vendors"]
        }"#;

        let merged: Value = serde_json::from_str(&merge_package_declaration(&ours, update)).unwrap();

        // The owner's edit survives.
        let interfaces = merged["requires"]["interfaces"].as_array().unwrap();
        assert!(
            interfaces.iter().any(|v| v == "mail"),
            "the capability the owner gave it survives the update: {interfaces:?}"
        );

        let ids: Vec<&str> = merged["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&"trade.permit_number"),
            "the question the owner wrote survives the update: {ids:?}"
        );

        // And something the owner never touched DOES get the update.
        assert!(
            ids.contains(&"finance.ap.terms"),
            "a new question from the package still arrives: {ids:?}"
        );
        let corrected = merged["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["id"] == "finance.ap.invoice_mailbox")
            .unwrap();
        assert_eq!(
            corrected["label"], "Which mailbox do bills arrive in?",
            "a question the owner never edited takes the package's correction"
        );
        assert_eq!(
            merged["ceiling"]["ledger.invoice.send"], "approval",
            "a new ceiling operation still arrives"
        );
        assert!(
            merged["subscribes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "parties.vendors"),
            "a new fact domain still arrives"
        );
        // The package's own plugin list was never the owner's to hold.
        assert_eq!(merged["requires"]["plugins"], serde_json::json!(["quickbooks"]));

        // Superseded, not deleted: the record still shows what the package said.
        assert_eq!(
            merged[PACKAGE_BASELINE]["requires"]["interfaces"],
            serde_json::json!(["ledger"]),
            "the baseline keeps the package's word"
        );
    }

    /// An entry the owner deleted stays deleted. Re-adding it on every update
    /// would be the same silent override in the other direction.
    #[test]
    fn an_entry_the_owner_deleted_is_not_quietly_restored() {
        let shipped = r#"{"requires": {"interfaces": ["ledger", "sms"]},
                           "ceiling": {"sms.message.send": "approval"}}"#;
        let ours = owner_edits(shipped, |fm| {
            fm["requires"]["interfaces"] = serde_json::json!(["ledger"]);
            fm["ceiling"] = serde_json::json!({});
        });

        let merged: Value =
            serde_json::from_str(&merge_package_declaration(&ours, shipped)).unwrap();

        assert_eq!(merged["requires"]["interfaces"], serde_json::json!(["ledger"]));
        assert!(
            merged["ceiling"].as_object().unwrap().is_empty(),
            "the ceiling entry the owner removed stays removed: {}",
            merged["ceiling"]
        );
    }

    /// An employee the owner never edited takes the update whole — an update
    /// that can change nothing is its own bug.
    #[test]
    fn an_untouched_employee_takes_the_update_whole() {
        let shipped = r#"{"requires": {"interfaces": ["ledger"]}, "inputs": [{"key": "a"}]}"#;
        let update = r#"{"requires": {"interfaces": ["ledger", "mail"]}, "inputs": [{"key": "b"}]}"#;

        let merged: Value =
            serde_json::from_str(&merge_package_declaration(shipped, update)).unwrap();

        assert_eq!(
            merged["requires"]["interfaces"],
            serde_json::json!(["ledger", "mail"])
        );
        assert_eq!(merged["inputs"], serde_json::json!([{"key": "b"}]));
    }

    /// The baseline is the package's word and never moves to the owner's, or a
    /// second update would hand the owner's earlier edits away.
    #[test]
    fn a_second_owner_save_does_not_move_the_baseline() {
        let shipped = r#"{"requires": {"interfaces": ["ledger"]}}"#;
        let once = owner_edits(shipped, |fm| {
            fm["requires"]["interfaces"] = serde_json::json!(["ledger", "mail"]);
        });
        let twice = {
            let pre: Value = serde_json::from_str(&once).unwrap();
            let mut after = pre.clone();
            after["requires"]["interfaces"] = serde_json::json!(["ledger", "mail", "sms"]);
            note_owner_edit(&mut after, &pre, OWNER_AUTHORED_FIELDS);
            after.to_string()
        };
        let v: Value = serde_json::from_str(&twice).unwrap();
        assert_eq!(
            v[PACKAGE_BASELINE]["requires"]["interfaces"],
            serde_json::json!(["ledger"]),
            "still the package's original word, not the owner's first edit"
        );

        // So both owner edits survive the update.
        let merged: Value =
            serde_json::from_str(&merge_package_declaration(&twice, shipped)).unwrap();
        let ifaces = merged["requires"]["interfaces"].as_array().unwrap();
        assert!(ifaces.iter().any(|v| v == "mail") && ifaces.iter().any(|v| v == "sms"));
    }

    /// The isolation toggle keeps riding through, since this merge is now the
    /// thing standing between a package's frontmatter and the row.
    #[test]
    fn the_isolation_toggle_still_survives_a_sync() {
        let ours = r#"{"memory": {"context_isolated": true}}"#;
        let incoming = r#"{"memory": {"context_isolated": false, "topics": []}}"#;
        let merged: Value =
            serde_json::from_str(&merge_package_declaration(ours, incoming)).unwrap();
        assert_eq!(merged["memory"]["context_isolated"], serde_json::json!(true));
        assert!(merged["memory"]["topics"].is_array(), "the package's own memory config still lands");
    }

    /// The owner's edits are mirrored to `{napp_path}/agent.json`, and the
    /// watcher hands that file straight back. That round trip must not be
    /// mistaken for a package update: if it advanced the baseline to the owner's
    /// own values, the next real update would read their edits as the package's
    /// and hand every one of them away. Mutation check: delete the
    /// `is_package_delivery` guard and the final assertion fails.
    #[test]
    fn our_own_file_coming_back_does_not_move_the_baseline() {
        let shipped = r#"{"requires": {"interfaces": ["ledger"]}}"#;
        let ours = owner_edits(shipped, |fm| {
            fm["requires"]["interfaces"] = serde_json::json!(["ledger", "mail"]);
        });

        // The watcher reads the file we just mirrored — it carries the baseline.
        let after_watcher = merge_package_declaration(&ours, &ours);
        let v: Value = serde_json::from_str(&after_watcher).unwrap();
        assert_eq!(
            v[PACKAGE_BASELINE]["requires"]["interfaces"],
            serde_json::json!(["ledger"]),
            "still the package's word after the round trip"
        );
        assert_eq!(
            v["requires"]["interfaces"],
            serde_json::json!(["ledger", "mail"]),
            "and the owner's edit is still there"
        );

        // Now a genuine package update, which carries no baseline.
        let merged: Value =
            serde_json::from_str(&merge_package_declaration(&after_watcher, shipped)).unwrap();
        assert!(
            merged["requires"]["interfaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "mail"),
            "the owner's edit still survives after a watcher round trip: {}",
            merged["requires"]["interfaces"]
        );
    }

    /// A question is the same question when its label changes: identity is the
    /// semantic id, so an owner's reword is an edit, not a second question.
    #[test]
    fn a_question_is_identified_by_its_semantic_id() {
        let shipped = r#"{"inputs": [{"id": "finance.ap.terms", "key": "terms", "label": "Terms?"}]}"#;
        let ours = owner_edits(shipped, |fm| {
            fm["inputs"] = serde_json::json!([
                {"id": "finance.ap.terms", "key": "net_terms", "label": "What terms do you give?"}
            ]);
        });
        let merged: Value =
            serde_json::from_str(&merge_package_declaration(&ours, shipped)).unwrap();
        let inputs = merged["inputs"].as_array().unwrap();
        assert_eq!(inputs.len(), 1, "one question, edited — not two: {inputs:?}");
        assert_eq!(inputs[0]["label"], "What terms do you give?");
    }
}
