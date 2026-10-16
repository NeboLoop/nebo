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

/// One owner notification: an Inbox item. `id` should be deterministic
/// when the event is retryable (e.g. `wf-approval:{run_id}`) — emission is
/// idempotent.
///
/// An item is one of two kinds: an update (a title, one sentence, a link
/// to the work) or a decision that needs the owner (`needs_you`), answered
/// right on its card with `options`, or on its own screen (its link) when
/// it has none. Every surface answers it through the one answer path
/// (`POST /notifications/{id}/answer`), which settles it ([`settle`]).
#[derive(Default)]
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
    /// A decision or question waiting on the owner.
    pub needs_you: bool,
    /// The answers it takes on its card, in order.
    pub options: &'a [db::models::ItemOption],
    /// The words on its link ("Orders sheet").
    pub link_label: Option<&'a str>,
    /// The session an employee's own question is answered into.
    pub reply_to: Option<&'a str>,
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
        if self.needs_you {
            item["needsYou"] = json!(true);
        }
        if let Some(label) = self.link_label {
            item["linkLabel"] = json!(label);
        }
        if let (Some(item), serde_json::Value::Object(extra)) = (item.as_object_mut(), extra) {
            item.extend(extra);
        }
        item
    }
}

/// Persist the Inbox row (idempotent) and broadcast the canonical payload.
/// Persistence is best-effort — the live broadcast is what surfaces the
/// notification, so a failed row write is logged, never fatal. Returns
/// whether the row is stored, for a caller that must not go on without it.
pub fn emit(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    n: &OwnerNotification,
) -> bool {
    // notifications FK to users(id) — resolve the real local user ("" violates it).
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    let link = n.link();
    let stored = match store.create_notification_if_not_exists(&db::models::NewNotification {
        id: n.id,
        user_id: &user_id,
        notification_type: n.kind,
        title: n.title,
        body: n.body,
        action_url: Some(&link),
        agent_id: n.agent_id,
        needs_you: n.needs_you,
        options: n.options,
        link_label: n.link_label,
        reply_to: n.reply_to,
    }) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(id = %n.id, error = %e, "owner notification row not persisted; broadcasting anyway");
            false
        }
    };

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
                "needsYou": n.needs_you,
                "options": n.options,
                "linkLabel": n.link_label,
                "status": db::models::Notification::status_of(n.needs_you, None, None),
            }),
        );
    }
    stored
}

/// The answers a staged proposal (`learn:`) takes on its card.
pub fn proposal_options() -> Vec<db::models::ItemOption> {
    vec![
        db::models::ItemOption { label: "Approve".into(), value: "yes".into() },
        db::models::ItemOption { label: "Decline".into(), value: "no".into() },
    ]
}

/// Settle a needs_you item: it leaves Needs you for History with the
/// answer in the owner's words and the surface that gave it, and every
/// open app hears it (`notification_settled`). The first answer from any
/// surface wins; a later one changes nothing and returns false. Called by
/// each item's own answer path (an ask's, a proposal's, a question's), so
/// an answer given anywhere settles the item everywhere.
pub fn settle(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    id: &str,
    answer: &str,
    via: Option<&str>,
) -> bool {
    match store.settle_notification(id, answer, via) {
        Ok(true) => {
            settled(broadcast, id, answer, via);
            true
        }
        Ok(false) => false,
        Err(e) => {
            tracing::warn!(id, error = %e, "Inbox item not settled");
            false
        }
    }
}

/// [`settle`] every open item whose id starts with `prefix`, all but `keep`:
/// an update notice for a version no longer on offer.
pub fn settle_all(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    prefix: &str,
    keep: Option<&str>,
    answer: &str,
) {
    match store.settle_notifications_like(prefix, keep, answer) {
        Ok(ids) => ids.iter().for_each(|id| settled(broadcast, id, answer, None)),
        Err(e) => tracing::warn!(prefix, error = %e, "Inbox items not settled"),
    }
}

/// Every open app hears an item was settled.
fn settled(broadcast: Option<&dyn Fn(&str, serde_json::Value)>, id: &str, answer: &str, via: Option<&str>) {
    if let Some(broadcast) = broadcast {
        broadcast(
            "notification_settled",
            json!({ "id": id, "answer": answer, "answeredVia": via, "status": "answered" }),
        );
    }
}

/// Restate an item the owner was already given, in place, when what it says
/// has changed (another duty held on the same need, the newest words of the
/// same proposal): the row keeps when it was made and whether it was read,
/// and the live Inbox takes the new words under the same id, quietly. False
/// when there is no such item (the owner dismissed it): it is never brought
/// back. A caller that mirrors the item restates its hub copy
/// ([`OwnerNotification::hub_item`]) only on true; the hub takes the same id
/// as an update, so nothing is pushed to a phone or emailed again.
pub fn restate(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    n: &OwnerNotification,
) -> bool {
    let link = n.link();
    match store.restate_notification(n.id, n.title, n.body, Some(&link)) {
        Ok(true) => {}
        Ok(false) => return false,
        Err(e) => {
            tracing::warn!(id = %n.id, error = %e, "owner notification not restated");
            return false;
        }
    }
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    if let (Some(broadcast), Ok(Some(row))) = (broadcast, store.get_notification(n.id, &user_id)) {
        broadcast(
            "notification_created",
            json!({
                "id": n.id,
                "type": n.kind,
                "title": n.title,
                "body": n.body,
                "actionUrl": link,
                "agentId": n.agent_id,
                "readAt": row.read_at,
                "createdAt": row.created_at,
                "needsYou": row.needs_you,
                "options": row.options,
                "linkLabel": row.link_label,
                "status": row.status,
            }),
        );
    }
    true
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

    /// The assistant's chat: the owner's one front door. Where everything
    /// waiting on him that he is not already in opens (a colleague's
    /// hand-off, a schedule, a background run): its card is relayed there.
    pub fn assistant() -> String {
        employee(crate::team_tool::PRIMARY_AGENT_ID)
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

    /// A team's own thread.
    pub fn team(team_id: &str) -> String {
        format!("/teams/{}", urlencoding::encode(team_id))
    }

    /// One of a team's projects: its team's page, opened on the project's
    /// thread (an app that knows no projects opens the team).
    pub fn project(team_id: &str, project_id: &str) -> String {
        format!("{}?project={}", team(team_id), urlencoding::encode(project_id))
    }

    /// One run of an employee's work.
    pub fn run(agent_id: &str, run_id: &str) -> String {
        format!("/{agent_id}/runs/{}", urlencoding::encode(run_id))
    }

    /// An employee's cases.
    pub fn cases(agent_id: &str) -> String {
        format!("/{agent_id}/cases")
    }

    /// One of an employee's cases, opened in its cases.
    pub fn case(agent_id: &str, case_id: &str) -> String {
        format!("/{agent_id}/cases?case={}", urlencoding::encode(case_id))
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
            assert_eq!(assistant(), "/assistant");
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
        store.get_notification(id, &user).unwrap().unwrap()
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
            (OwnerNotification { id: "wf-fail:r1", kind: "error", title: "t", body: None, action_url: Some(&place), agent_id: Some("emp"), loud: false, ..Default::default() }, "/emp/runs/r1"),
            (OwnerNotification { id: "backup-failed:7", kind: "error", title: "t", body: None, action_url: None, agent_id: None, loud: true, ..Default::default() }, "/inbox?m=backup-failed%3A7"),
        ];
        for (n, want) in &cases {
            emit(&store, Some(&broadcast), n);
            assert_eq!(n.link(), *want);
            assert_eq!(row(&store, n.id).action_url.as_deref(), Some(*want), "{}: the Inbox row", n.id);
            assert_eq!(seen.lock().unwrap().last().unwrap()["actionUrl"], *want, "{}: the live banner", n.id);
            assert_eq!(n.hub_item(serde_json::json!({}))["link"], *want, "{}: the hub copy", n.id);
        }
    }

    /// An item told again with new words is restated in place, quietly: the
    /// row keeps whether it was read and when it was made, the live Inbox
    /// takes the words under the same id and never as a banner. An item the
    /// owner dismissed is not brought back.
    #[test]
    fn a_restated_item_changes_its_words_in_place_and_a_dismissed_one_stays_gone() {
        let (_dir, store) = store();
        let seen = std::sync::Mutex::new(Vec::new());
        let broadcast = |ev: &str, payload: serde_json::Value| seen.lock().unwrap().push((ev.to_string(), payload));
        let first = OwnerNotification { id: "need:1", kind: "warning", title: "Ava needs a telephony plugin", body: Some("one duty"), action_url: Some("/settings/plugins"), agent_id: Some("ava"), loud: true, ..Default::default() };
        emit(&store, Some(&broadcast), &first);
        let user = store.ensure_local_user_id().unwrap();
        store.mark_notification_read("need:1", &user).unwrap();
        let made = row(&store, "need:1");

        let again = OwnerNotification { body: Some("two duties"), ..first };
        assert!(restate(&store, Some(&broadcast), &again));
        let now = row(&store, "need:1");
        assert_eq!(now.body.as_deref(), Some("two duties"));
        assert_eq!((now.read_at, now.created_at), (made.read_at, made.created_at), "read stays read; made stays when it was made");
        let (ev, payload) = seen.lock().unwrap().last().unwrap().clone();
        assert_eq!(ev, "notification_created", "a restatement is never a banner");
        assert_eq!((payload["id"].as_str(), payload["body"].as_str()), (Some("need:1"), Some("two duties")));
        assert_eq!(store.list_user_notifications(&user, db::models::InboxView::All, None, 50, 0).unwrap().len(), 1, "still one item");

        store.archive_notification("need:1", &user).unwrap();
        let heard = seen.lock().unwrap().len();
        assert!(!restate(&store, Some(&broadcast), &again), "dismissed: not restated");
        assert!(store.list_user_notifications(&user, db::models::InboxView::All, None, 50, 0).unwrap().is_empty(), "and not brought back");
        assert_eq!(seen.lock().unwrap().len(), heard, "nothing is broadcast for it");
    }

    /// The hub copy is the item itself plus only what the hub alone
    /// carries; an item with no employee names none.
    #[test]
    fn the_hub_copy_is_the_item() {
        let n = OwnerNotification { id: "need:1", kind: "warning", title: "Ava needs Gmail connected", body: Some("b"), action_url: Some("/ava/settings/accounts?plugin=gmail"), agent_id: Some("ava"), loud: false, ..Default::default() };
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

    /// The two kinds of item, through the one writer: an update lists in
    /// Updates; a needs_you item keeps its answers and lists in Needs you
    /// until it is settled, then in History with the answer and where it
    /// came from. The first answer wins. Nothing is deleted: an item put
    /// away is History, and told again it comes back fresh.
    #[test]
    fn an_item_is_an_update_or_needs_you_and_an_answer_moves_it_to_history() {
        use db::models::{InboxView, ItemOption};
        let (_dir, store) = store();
        let user = store.ensure_local_user_id().unwrap();
        let ids = |view| -> Vec<String> {
            store.list_user_notifications(&user, view, None, 50, 0).unwrap().into_iter().map(|n| n.id).collect()
        };
        let options = [
            ItemOption { label: "Lumen · $14.20".into(), value: "lumen".into() },
            ItemOption { label: "Northfield · $16.20".into(), value: "northfield".into() },
        ];
        let ask = OwnerNotification {
            id: "q:1", kind: "info", title: "Which supplier?", body: Some("Stock runs out in 6 days."),
            agent_id: Some("store"), needs_you: true, options: &options, reply_to: Some("agent:store:cron:x"),
            ..Default::default()
        };
        let update = OwnerNotification {
            id: "u:1", kind: "info", title: "Shopify orders synced", body: Some("42 new orders."),
            action_url: Some("/store/runs/r1"), agent_id: Some("store"), link_label: Some("Orders sheet"),
            ..Default::default()
        };
        assert!(emit(&store, None, &ask) && emit(&store, None, &update));
        assert_eq!(ids(InboxView::NeedsYou), ["q:1"]);
        assert_eq!(ids(InboxView::Updates), ["u:1"]);
        let q = row(&store, "q:1");
        assert_eq!((q.status.as_str(), q.options.clone(), q.reply_to.as_deref()), ("open", options.to_vec(), Some("agent:store:cron:x")));
        assert_eq!(row(&store, "u:1").link_label.as_deref(), Some("Orders sheet"));

        let seen = std::sync::Mutex::new(Vec::new());
        let broadcast = |ev: &str, payload: serde_json::Value| seen.lock().unwrap().push((ev.to_string(), payload));
        assert!(settle(&store, Some(&broadcast), "q:1", "Lumen · $14.20", Some("mobile")));
        assert!(!settle(&store, Some(&broadcast), "q:1", "Northfield · $16.20", Some("web")), "the first answer wins");
        let q = row(&store, "q:1");
        assert_eq!((q.status.as_str(), q.answer.as_deref(), q.answered_via.as_deref()), ("answered", Some("Lumen · $14.20"), Some("mobile")));
        assert!(q.read_at.is_some());
        assert_eq!(seen.lock().unwrap().len(), 1, "one settle heard");
        assert!(ids(InboxView::NeedsYou).is_empty());
        assert_eq!(ids(InboxView::History), ["q:1"]);

        store.archive_notification("u:1", &user).unwrap();
        assert_eq!(row(&store, "u:1").status, "archived", "put away, never deleted");
        assert!(ids(InboxView::Updates).is_empty() && ids(InboxView::History).contains(&"u:1".to_string()));
        assert!(emit(&store, None, &update), "told again");
        assert_eq!((row(&store, "u:1").status.as_str(), row(&store, "u:1").read_at), ("update", None), "it comes back fresh");
    }

}
