//! Owner notification emission — the ONE persist+broadcast sequence.
//!
//! Eleven hand-built copies existed (audit 2026-08-22) and had drifted on
//! three axes: payload field names (`actionUrl` vs `link`, `body` vs
//! `message`), payload completeness (`agentId`/`readAt`/`createdAt` present
//! at some sites only), and idempotency (deterministic ids written with the
//! non-deduped insert hard-error on retry). The comments at two of the copies
//! record the same empty-row bug being fixed one copy at a time.
//!
//! The TWO event names are deliberate and stay: they are the client's
//! loudness contract, not drift —
//! - `notification` (loud): Inbox row + toast + native OS banner
//!   (`listeners.ts` fires the Tauri notification client-side).
//! - `notification_created` (quiet): Inbox row only (the bell).

use serde_json::json;

/// One owner notification. `id` should be deterministic when the event is
/// retryable (e.g. `wf-approval:{run_id}`) — emission is idempotent.
pub struct OwnerNotification<'a> {
    pub id: &'a str,
    /// Inbox row type: "info" | "warning" | "error" | "approval" | ...
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    /// The place the item is about ([`link`]). `None` when it is about no
    /// other place: it opens as itself, read (and decided) in the Inbox.
    pub action_url: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    /// `true` = bell + toast + native OS banner; `false` = bell only.
    pub loud: bool,
}

impl OwnerNotification<'_> {
    /// Where the item opens, on every surface: the place it is about, else
    /// the item itself in the Inbox — never nowhere.
    pub fn link(&self) -> String {
        self.action_url.map(str::to_string).unwrap_or_else(|| link::inbox_item(self.id))
    }

    /// The same item for the owner's hub Inbox, which pushes it to his
    /// phone: one id, kind, words and [`Self::link`] wherever it is shown,
    /// so a push tap, a phone Inbox row and the desktop open one place.
    /// `extra` adds what only the hub copy carries (its chat, its answer
    /// buttons).
    pub fn hub_item(&self, extra: serde_json::Value) -> serde_json::Value {
        let mut item = json!({
            "id": self.id,
            "type": self.kind,
            "title": self.title,
            "body": self.body,
            "link": self.link(),
        });
        if let Some(agent_id) = self.agent_id.filter(|a| !a.is_empty()) {
            item["agentId"] = json!(agent_id);
        }
        if let (Some(item), serde_json::Value::Object(extra)) = (item.as_object_mut(), extra) {
            item.extend(extra);
        }
        item
    }
}

/// Persist the Inbox row (idempotent) and broadcast the canonical payload.
/// Persistence is best-effort — the live broadcast is what surfaces the
/// notification, so a failed row write is logged, never fatal.
pub fn emit(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    n: &OwnerNotification,
) {
    // notifications FK to users(id) — resolve the real local user ("" violates it).
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    let link = n.link();
    if let Err(e) = store.create_notification_if_not_exists(
        n.id,
        &user_id,
        n.kind,
        n.title,
        n.body,
        Some(&link),
        None,
        n.agent_id,
    ) {
        tracing::warn!(id = %n.id, error = %e, "owner notification row not persisted; broadcasting anyway");
    }

    if let Some(broadcast) = broadcast {
        broadcast(
            if n.loud { "notification" } else { "notification_created" },
            json!({
                "id": n.id,
                "type": n.kind,
                "title": n.title,
                "body": n.body,
                "actionUrl": link,
                "agentId": n.agent_id,
                "readAt": null,
                "createdAt": chrono::Utc::now().timestamp(),
            }),
        );
    }
}

/// Where an owner item opens: its Inbox row's `action_url`, and the `link`
/// of the same item mirrored to the owner's hub Inbox and pushed to his
/// phone ([`OwnerNotification::hub_item`]). A path in the app's own route
/// space, so every surface reads the same address: the desktop navigates to
/// it, the web Inbox opens it through the tunnel, and the phone maps it to
/// its screen (mobile `lib/api/inbox_route.dart`). Every item that is about
/// a place names that place here, never somewhere generic; this module is
/// the ONE builder of these addresses.
pub mod link {
    /// An employee: its latest conversation (a hire, or an item about the
    /// employee with no conversation of its own).
    pub fn employee(agent_id: &str) -> String {
        format!("/{agent_id}")
    }

    /// A conversation: `/{agent}/threads/{chat}` — where a card the owner
    /// answers sits. The employee when the chat is not known.
    pub fn chat(agent_id: &str, chat_id: &str) -> String {
        if chat_id.is_empty() {
            return employee(agent_id);
        }
        format!("/{agent_id}/threads/{}", urlencoding::encode(chat_id))
    }

    /// The conversation a session is holding now (the session key is its
    /// name), or "" when there is no such session.
    pub fn conversation(store: &db::Store, session_key: &str) -> String {
        store
            .get_session_by_name(session_key)
            .ok()
            .flatten()
            .map(|s| store.resolve_session_chat_id(&s.id))
            .unwrap_or_default()
    }

    /// The [`conversation`] a session is holding, as a [`chat`].
    pub fn session_chat(store: &db::Store, agent_id: &str, session_key: &str) -> String {
        chat(agent_id, &conversation(store, session_key))
    }

    /// One run of an employee's work.
    pub fn run(agent_id: &str, run_id: &str) -> String {
        format!("/{agent_id}/runs/{}", urlencoding::encode(run_id))
    }

    /// An employee's cases.
    pub fn cases(agent_id: &str) -> String {
        format!("/{agent_id}/cases")
    }

    /// An Inbox item opened as itself in the Inbox's reader: a proposal the
    /// owner reads in full before he decides (`learn:<id>`), or a notice
    /// about no other place.
    pub fn inbox_item(item_id: &str) -> String {
        format!("/inbox?m={}", urlencoding::encode(item_id))
    }

    /// An employee's connections, with `plugin` first when one is named:
    /// where "X needs Y connected" is fixed.
    pub fn accounts(agent_id: &str, plugin: Option<&str>) -> String {
        match plugin.filter(|p| !p.is_empty()) {
            Some(p) => format!("/{agent_id}/settings/accounts?plugin={}", urlencoding::encode(p)),
            None => format!("/{agent_id}/settings/accounts"),
        }
    }

    /// One section of an employee's settings (its workflows, its general
    /// page).
    pub fn employee_settings(agent_id: &str, section: &str) -> String {
        format!("/{agent_id}/settings/{section}")
    }

    /// Settings → Plugins, where a plugin is added or turned on.
    pub fn plugins() -> String {
        "/settings/plugins".to_string()
    }

    /// Settings → Updates, at one package.
    pub fn update(artifact_id: &str) -> String {
        format!("/settings/updates?artifact={}", urlencoding::encode(artifact_id))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_place_has_one_address() {
            assert_eq!(chat("emp", "c-1"), "/emp/threads/c-1");
            assert_eq!(chat("emp", ""), "/emp");
            assert_eq!(employee("emp"), "/emp");
            assert_eq!(cases("emp"), "/emp/cases");
            assert_eq!(employee_settings("emp", "workflows"), "/emp/settings/workflows");
            assert_eq!(plugins(), "/settings/plugins");
            assert_eq!(run("emp", "r 1"), "/emp/runs/r%201");
            assert_eq!(inbox_item("learn:abc"), "/inbox?m=learn%3Aabc");
            assert_eq!(accounts("emp", Some("gmail")), "/emp/settings/accounts?plugin=gmail");
            assert_eq!(accounts("emp", None), "/emp/settings/accounts");
            assert_eq!(update("google-sheets"), "/settings/updates?artifact=google-sheets");
        }

        #[test]
        fn a_session_opens_its_conversation() {
            let dir = tempfile::tempdir().unwrap();
            let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
            assert_eq!(session_chat(&store, "emp", "agent:emp:web"), "/emp", "no session: the employee");
            let s = store.create_session("s1", Some("agent:emp:web"), Some("agent"), Some("emp"), None).unwrap();
            let expected = format!("/emp/threads/{}", urlencoding::encode(&store.resolve_session_chat_id(&s.id)));
            assert_eq!(session_chat(&store, "emp", "agent:emp:web"), expected);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        (dir, store)
    }

    fn row(store: &db::Store, id: &str) -> db::models::Notification {
        let user = store.ensure_local_user_id().unwrap();
        store.list_user_notifications(&user, 50, 0).unwrap().into_iter().find(|n| n.id == id).unwrap()
    }

    /// An item about a place opens there, and one about no other place
    /// opens as itself in the Inbox — never nowhere. The row here, the live
    /// banner and the hub copy the phone is pushed carry the same link.
    #[test]
    fn every_item_opens_somewhere_and_the_same_place_everywhere() {
        let (_dir, store) = store();
        let seen = std::sync::Mutex::new(Vec::new());
        let broadcast = |_: &str, payload: serde_json::Value| seen.lock().unwrap().push(payload);
        let place = link::run("emp", "r1");
        let cases = [
            (OwnerNotification { id: "wf-fail:r1", kind: "error", title: "t", body: None, action_url: Some(&place), agent_id: Some("emp"), loud: false }, "/emp/runs/r1"),
            (OwnerNotification { id: "backup-failed:7", kind: "error", title: "t", body: None, action_url: None, agent_id: None, loud: true }, "/inbox?m=backup-failed%3A7"),
        ];
        for (n, want) in &cases {
            emit(&store, Some(&broadcast), n);
            assert_eq!(n.link(), *want);
            assert_eq!(row(&store, n.id).action_url.as_deref(), Some(*want), "{}: the Inbox row", n.id);
            assert_eq!(seen.lock().unwrap().last().unwrap()["actionUrl"], *want, "{}: the live banner", n.id);
            assert_eq!(n.hub_item(serde_json::json!({}))["link"], *want, "{}: the hub copy", n.id);
        }
    }

    /// The hub copy is the item itself plus only what the hub alone
    /// carries; an item with no employee names none.
    #[test]
    fn the_hub_copy_is_the_item() {
        let n = OwnerNotification { id: "need:1", kind: "warning", title: "Ava needs Gmail connected", body: Some("b"), action_url: Some("/ava/settings/accounts?plugin=gmail"), agent_id: Some("ava"), loud: false };
        let item = n.hub_item(serde_json::json!({ "chatId": "c1" }));
        assert_eq!(
            item,
            serde_json::json!({
                "id": "need:1", "type": "warning", "title": "Ava needs Gmail connected", "body": "b",
                "link": "/ava/settings/accounts?plugin=gmail", "agentId": "ava", "chatId": "c1",
            })
        );
        let n = OwnerNotification { agent_id: None, ..n };
        assert!(n.hub_item(serde_json::json!({})).get("agentId").is_none());
    }
}
