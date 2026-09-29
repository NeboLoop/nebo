//! Outside doors: the ways someone who is not the owner reaches an
//! employee — a phone line and its texts, a QR code, a Slack, Discord or
//! Teams channel, the loop, a webhook.
//!
//! Owner rule (09-25): an employee connected to any of them is a multi-chat
//! employee, and every outside caller or conversation is a chat of its own.
//! Two strangers never share a thread, and one's words never land in the
//! other's running turn. The routing below holds that whatever the stored
//! flag says; the flag (`entity_config.multi_chat`) is the employee's
//! recorded property, turned on by every door's writer
//! (`Store::mark_multi_chat`) and refused off while a door is bound.

/// Someone outside on the far end of an agent-space message. NeboAI
/// delivers texts and voicemails for an employee's phone line, and chat
/// webhooks, into the employee's ONE agent-space conversation; the envelope's
/// `platformData` says who it is from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutsideParty {
    /// A text or a voicemail: the line it came in on and the number it came
    /// from.
    Phone { line: String, from: String },
    /// One webhook delivery.
    Webhook,
}

/// The outside party an agent-space message's content names, if any. A
/// webhook minted for a workflow runs that workflow and never a chat, so it
/// is no party to one.
pub(crate) fn outside_party(content: &str) -> Option<OutsideParty> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let platform = value.get("platformData")?;
    match platform.get("channel").and_then(|c| c.as_str())? {
        "phone" => Some(OutsideParty::Phone {
            line: platform.get("line").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            from: platform.get("from").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        }),
        "webhook" if platform.get("workflowName").and_then(|w| w.as_str()).is_none_or(str::is_empty) => {
            Some(OutsideParty::Webhook)
        }
        _ => None,
    }
}

/// Where an agent-space message runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSpaceRoute {
    pub session_key: String,
    /// The outside party's own thread: its chat id and, for a phone number,
    /// the title that names it (a webhook's thread is named by the
    /// auto-namer). `None` for the owner and loop members.
    pub thread: Option<(String, Option<String>)>,
}

/// The session an agent-space message runs in. The owner and loop members
/// keep the session `member_key` chooses; an outside party gets a thread of
/// the employee's own: one per phone number on a line (its texts and
/// voicemails read as one conversation), one per webhook delivery.
/// `agent_row` is the employee's local id (`assistant` for the primary);
/// `delivery_id` is the message's id.
pub(crate) fn agent_space_route(
    agent_row: &str,
    party: Option<&OutsideParty>,
    delivery_id: &str,
    member_key: impl FnOnce() -> String,
) -> AgentSpaceRoute {
    let Some(party) = party else {
        return AgentSpaceRoute { session_key: member_key(), thread: None };
    };
    let digits = |s: &str| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
    let delivery = || match delivery_id {
        "" => uuid::Uuid::new_v4().to_string(),
        id => id.to_string(),
    };
    let (chat_id, title) = match party {
        // A number that cannot be told apart from another (withheld) is
        // one delivery's thread, never a shared "unknown caller" one.
        OutsideParty::Phone { line, from } if !digits(from).is_empty() => (
            format!("phone-{agent_row}-{}-{}", digits(line), digits(from)),
            Some(format!("Messages from {}", phone_display(from))),
        ),
        OutsideParty::Phone { .. } => (format!("phone-{}", delivery()), Some("Messages from a withheld number".to_string())),
        OutsideParty::Webhook => (format!("webhook-{}", delivery()), None),
    };
    AgentSpaceRoute {
        session_key: types::keyparser::build_agent_session_key(agent_row, &format!("thread:{chat_id}")),
        thread: Some((chat_id, title)),
    }
}

/// Why an entity-config patch may not turn multi-chat off, or `None` when it
/// may. It may not while any outside door is bound: `doors` are the
/// employee's bound doors (`Store::outside_doors`, plus `phonecall` for a
/// line NeboAI holds for it), `name_of` a plugin's display name.
pub(crate) fn multi_chat_lock(
    patch: &serde_json::Value,
    doors: &[String],
    name_of: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let off = patch.get("multiChat").is_some_and(|v| match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::Number(n) => n.as_i64() == Some(0),
        _ => false,
    });
    if !off || doors.is_empty() {
        return None;
    }
    let labels: Vec<String> = doors
        .iter()
        .map(|slug| match slug.as_str() {
            "phonecall" => "a phone line".to_string(),
            "loop" => "the loop".to_string(),
            other => name_of(other).unwrap_or_else(|| other.to_string()),
        })
        .collect();
    let list = match labels.as_slice() {
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
        [] => unreachable!("checked above"),
    };
    Some(format!(
        "Multi-chat stays on while this employee is connected to {list}: everyone who reaches it from outside gets their own chat, so one caller never sees another's. Disconnect {} first.",
        if labels.len() == 1 { "it" } else { "them" }
    ))
}

/// A phone number the way a phone shows it: "(801) 023-2342" for a North
/// American number, anything else as given.
pub(crate) fn phone_display(raw: &str) -> String {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 11 && digits.starts_with('1') {
        format!("({}) {}-{}", &digits[1..4], &digits[4..7], &digits[7..])
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sms(line: &str, from: &str) -> String {
        serde_json::json!({
            "text": "Is the unit still available?",
            "platformData": {"channel": "phone", "kind": "sms", "line": line, "from": from},
        })
        .to_string()
    }

    fn route(content: &str, delivery: &str) -> AgentSpaceRoute {
        agent_space_route("desk", outside_party(content).as_ref(), delivery, || "agent:desk:web".to_string())
    }

    /// Texts from two numbers never share a thread, nor the owner's; a
    /// number that texts again lands in its own thread; a voicemail from it
    /// joins that thread; each webhook delivery is a thread of its own; the
    /// owner's own message keeps the owner's session.
    #[test]
    fn every_outside_party_on_the_agent_space_is_its_own_thread() {
        let a = route(&sms("+18015550100", "+18015550142"), "m1");
        let b = route(&sms("+18015550100", "+18015550199"), "m2");
        assert_ne!(a.session_key, "agent:desk:web", "a texter never runs in the owner's session");
        assert_ne!(a.session_key, b.session_key, "two numbers, two threads");
        assert_eq!(route(&sms("+18015550100", "+18015550142"), "m3"), a, "the same number again, the same thread");
        let voicemail = serde_json::json!({
            "text": "Voicemail",
            "platformData": {"channel": "phone", "line": "+18015550100", "from": "+18015550142", "callSid": "CA1"},
        })
        .to_string();
        assert_eq!(route(&voicemail, "m4").session_key, a.session_key);
        let (chat, title) = a.thread.clone().expect("a texter's thread");
        assert_eq!(a.session_key, format!("agent:desk:thread:{chat}"));
        assert_eq!(title.as_deref(), Some("Messages from (801) 555-0142"));

        let hook = serde_json::json!({"text": "New lead", "platformData": {"channel": "webhook", "endpointId": "e1"}}).to_string();
        let h1 = route(&hook, "d1");
        let h2 = route(&hook, "d2");
        assert_ne!(h1.session_key, h2.session_key, "two deliveries, two threads");
        assert_ne!(h1.session_key, "agent:desk:web");

        let workflow_hook = serde_json::json!({"text": "Order", "platformData": {"channel": "webhook", "workflowName": "intake"}}).to_string();
        assert_eq!(outside_party(&workflow_hook), None, "a workflow webhook runs its workflow, no chat");

        let owner = serde_json::json!({"text": "How was today?"}).to_string();
        assert_eq!(route(&owner, "m5"), AgentSpaceRoute { session_key: "agent:desk:web".into(), thread: None });
    }

    /// Multi-chat cannot be switched off — by false, 0 or clearing it —
    /// while a door is bound, and the refusal names the doors; switching it
    /// on, or off with no door, is allowed.
    #[test]
    fn multi_chat_cannot_be_switched_off_while_a_door_is_bound() {
        let path = std::env::temp_dir().join(format!("nebo-outside-{}.db", uuid::Uuid::new_v4()));
        let store = db::Store::new(&path.to_string_lossy()).unwrap();
        for id in ["desk", "quiet"] {
            store.create_agent(id, None, id, "", "", "{}", None, None).unwrap();
        }
        store.enable_channel_binding("desk", "slack").unwrap();
        store.enable_channel_binding("desk", "phonecall").unwrap();
        let name_of = |slug: &str| (slug == "slack").then(|| "Slack".to_string());
        let doors = store.outside_doors("desk").unwrap();

        for off in [serde_json::json!(false), serde_json::json!(0), serde_json::Value::Null] {
            let refusal = multi_chat_lock(&serde_json::json!({ "multiChat": off }), &doors, name_of)
                .expect("refused while bound");
            assert!(refusal.contains("Slack") && refusal.contains("a phone line"), "{refusal}");
            assert!(refusal.contains("own chat"), "the refusal says why: {refusal}");
        }
        assert_eq!(multi_chat_lock(&serde_json::json!({ "multiChat": 1 }), &doors, name_of), None);
        assert_eq!(multi_chat_lock(&serde_json::json!({ "modelPreference": "janus/fast" }), &doors, name_of), None);
        let quiet = store.outside_doors("quiet").unwrap();
        assert_eq!(multi_chat_lock(&serde_json::json!({ "multiChat": 0 }), &quiet, name_of), None);
    }

    /// A text and a call from different numbers to one employee, with the
    /// owner's own chat open and recent: the text runs in its number's
    /// thread, the call in a fresh one (`handlers::voice`), and neither in
    /// the owner's chat or the other's.
    #[test]
    fn a_text_and_a_call_from_different_numbers_do_not_share() {
        let path = std::env::temp_dir().join(format!("nebo-outside-{}.db", uuid::Uuid::new_v4()));
        let store = db::Store::new(&path.to_string_lossy()).unwrap();
        store.create_chat_for_session("owner-chat", "agent:desk:web", "Chat 1", None).unwrap();
        store.create_chat_message("o1", "owner-chat", "user", "Draft the listing.", None).unwrap();

        let text = route(&sms("+18015550100", "+18015550142"), "m1");
        if let Some((chat, title)) = &text.thread {
            store.create_chat_for_session(chat, &text.session_key, title.as_deref().unwrap_or("New Chat"), None).unwrap();
        }
        let recent = store.list_recent_agent_chats("desk", 8).unwrap();
        let now = chrono::Utc::now().timestamp();
        let busy = |k: &str| k == text.session_key;
        let call_chat = crate::handlers::voice::pick_voice_chat(true, &recent, now, busy)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // The chat each lands in: its own thread, else the one its session
        // already writes to (the owner's).
        let text_chat = text.thread.as_ref().map(|(c, _)| c.clone()).unwrap_or_else(|| "owner-chat".to_string());
        assert_ne!(text_chat, "owner-chat", "the text is not in the owner's chat");
        assert_ne!(call_chat, "owner-chat", "the call is not in the owner's chat");
        assert_ne!(call_chat, text_chat, "the call is not in the texter's thread");
    }
}
