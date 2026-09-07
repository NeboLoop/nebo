//! The effect ledger on the send path. Every customer-facing send — a text
//! from the employee's line, a message handed to Messages.app, a typed
//! `mail.message.send` or `sms.message.send` through a plugin — passes
//! through `guarded_send`, and there is no other way to send.
//!
//! The row is written BEFORE the attempt under a key derived from the run,
//! the operation, and the exact input (I-6). So:
//! - the same send asked for again in the same run (a relaunched turn, a
//!   model that repeats itself) finds a completed row and is not sent again;
//! - a send whose outcome is unknown (the provider was reached, no answer
//!   came back) stays pending, is never retried by anyone, and the owner is
//!   told with the ledger entry to check;
//! - a send the provider refused, or that failed before anything left the
//!   machine, is failed and may be tried again.
//!
//! The OUTCOME IS TYPED BY THE ADAPTER that talked to the provider — the hub
//! client's error type, the plugin's declared outcome, the app bridge's
//! result. The engine never infers whether a message went out from the
//! words in an error. An adapter that cannot say reports Unknown.
//!
//! Unknown-outcome policy is per provider. None of today's providers gives
//! an idempotency key on send, so every one of them HOLDS on unknown.

use std::future::Future;
use std::hash::{Hash, Hasher};

use db::Store;

use crate::origin::ToolContext;
use crate::ToolResult;

/// What the provider said, as the adapter that spoke to it reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// The provider confirmed acceptance. The message the employee sees,
    /// and the provider's reference if it gave one.
    Sent(String, Option<String>),
    /// The provider answered and refused. Nothing went out; may be tried
    /// again differently.
    ConfirmedFailure(String),
    /// Failed before anything left this machine (bad input, no line, no
    /// bridge). Nothing went out; may be tried again.
    PreSendFailure(String),
    /// The provider was asked and did not answer, or the adapter cannot
    /// say. Never retried; the owner decides.
    Unknown(String),
}

impl SendOutcome {
    /// The contract a plugin's send operation reports on stdout:
    /// `{"outcome": "confirmed_success" | "confirmed_failure" |
    /// "retryable_pre_send_failure" | "unknown_outcome", "message": "...",
    /// "reference": "..."}`. A plugin that says nothing typed is Unknown —
    /// a send whose fate nobody vouched for is a send that may have gone.
    pub fn from_plugin_output(output: &str) -> SendOutcome {
        let json: Option<serde_json::Value> = last_json_object(output).and_then(|s| serde_json::from_str(s).ok());
        let Some(v) = json else {
            return SendOutcome::Unknown(format!("the plugin reported no typed outcome: {}", output.chars().take(200).collect::<String>()));
        };
        let message = v["message"].as_str().map(str::to_string).unwrap_or_else(|| output.trim().to_string());
        match v["outcome"].as_str() {
            Some("confirmed_success") => SendOutcome::Sent(message, v["reference"].as_str().map(str::to_string)),
            Some("confirmed_failure") => SendOutcome::ConfirmedFailure(message),
            Some("retryable_pre_send_failure") => SendOutcome::PreSendFailure(message),
            Some("unknown_outcome") => SendOutcome::Unknown(message),
            _ => SendOutcome::Unknown(format!("the plugin reported no typed outcome: {message}")),
        }
    }
}

/// The last `{…}` in a string, for plugins that print a line of prose
/// before their JSON.
fn last_json_object(s: &str) -> Option<&str> {
    let end = s.rfind('}')?;
    let mut depth = 0i32;
    for (i, c) in s[..=end].char_indices().rev() {
        match c {
            '}' => depth += 1,
            '{' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[i..=end]);
                }
            }
            _ => {}
        }
    }
    None
}

/// What to do when a provider's outcome is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUnknown {
    /// Hold: stay pending, tell the owner, never retry.
    Hold,
    /// Retry under the provider's own idempotency key.
    Retry,
}

/// The provider capability table. Every provider Nebo sends through today
/// accepts a message without an idempotency key, so an unknown outcome may
/// already be a delivered message: hold.
pub fn on_unknown(provider: &str) -> OnUnknown {
    match provider {
        // hub SMS line, Messages.app, and every plugin-backed mail/sms port
        _ => OnUnknown::Hold,
    }
}

/// The typed ports that put words in front of a customer.
pub fn is_customer_send(operation: &str) -> bool {
    let op = operation.rsplit('.').take(3).collect::<Vec<_>>();
    matches!(op.as_slice(), ["send", "message", "mail" | "sms"])
}

/// The run this send belongs to: the workflow run when the session is a
/// workflow session, otherwise the session itself.
pub fn run_ref(ctx: &ToolContext) -> String {
    crate::origin::workflow_run_id(&ctx.session_key)
        .map(str::to_string)
        .unwrap_or_else(|| ctx.session_key.clone())
}

fn agent_of(ctx: &ToolContext) -> Option<String> {
    ctx.session_key
        .strip_prefix("agent:")
        .and_then(|s| s.split(':').next())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The key one exact send has: same run, same operation, same input.
pub fn send_key(run_ref: &str, operation: &str, input: &serde_json::Value) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    operation.hash(&mut h);
    input.to_string().hash(&mut h);
    format!("send:{run_ref}:{operation}:{:016x}", h.finish())
}

/// Perform one customer-facing send through the ledger.
pub async fn guarded_send<F, Fut>(
    store: &Store,
    ctx: &ToolContext,
    class: &str,
    provider: &str,
    operation: &str,
    input: &serde_json::Value,
    send: F,
) -> ToolResult
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = SendOutcome>,
{
    let run = run_ref(ctx);
    let key = send_key(&run, operation, input);
    let id = match store.engine_effect_pending(&run, class, &key, provider, "") {
        Ok(id) => id,
        Err(e) => return ToolResult::error(format!("could not record the send before attempting it; not sent: {e}")),
    };
    let effect = match store.engine_get_effect(id) {
        Ok(Some(e)) => e,
        _ => return ToolResult::error("could not read the send record; not sent"),
    };
    match effect.state.as_str() {
        "completed" => {
            return ToolResult::ok(format!(
                "Already sent earlier in this run ({operation}); not sent again. {}",
                effect.provider_ref.map(|r| format!("Provider reference: {r}.")).unwrap_or_default()
            ));
        }
        "pending" if effect.attempts > 0 => {
            // Attempted before, outcome unknown: it may already have gone out.
            tell_owner(store, ctx, id, provider, operation, "its outcome is still unknown");
            return ToolResult::error(format!(
                "This exact send ({operation}) was attempted before and its outcome is unknown, so it was NOT sent again. The owner has been asked to confirm it with the provider. Do not retry it."
            ));
        }
        _ => {}
    }
    let now = chrono::Utc::now().timestamp();
    if let Err(e) = store.engine_effect_attempted(id) {
        return ToolResult::error(format!("could not record the attempt; not sent: {e}"));
    }
    match send().await {
        SendOutcome::Sent(msg, provider_ref) => {
            let _ = store.engine_effect_completed(id, provider_ref.as_deref(), Some(&msg), now);
            ToolResult::ok(msg)
        }
        SendOutcome::ConfirmedFailure(why) | SendOutcome::PreSendFailure(why) => {
            let _ = store.engine_effect_failed(id, &why, now);
            ToolResult::error(why)
        }
        SendOutcome::Unknown(why) => match on_unknown(provider) {
            OnUnknown::Hold => {
                tell_owner(store, ctx, id, provider, operation, &why);
                ToolResult::error(format!(
                    "The send ({operation}) was attempted but the outcome is unknown: {why}. It was NOT retried, because it may already have been delivered. The owner has been asked to confirm it. Do not retry it."
                ))
            }
            OnUnknown::Retry => {
                let _ = store.engine_effect_failed(id, &why, now);
                ToolResult::error(format!("The send failed and may be retried: {why}"))
            }
        },
    }
}

fn tell_owner(store: &Store, ctx: &ToolContext, effect_id: i64, provider: &str, operation: &str, why: &str) {
    let user = store.ensure_local_user_id().unwrap_or_default();
    let body = format!(
        "A {operation} through {provider} was attempted and {why}. It was not retried, because it may already have been delivered. Check the provider's sent items; ledger entry #{effect_id}."
    );
    let agent = agent_of(ctx);
    let _ = store.create_notification_if_not_exists(
        &format!("attention:effect:{effect_id}"),
        &user,
        "needs_attention",
        "A send could not be confirmed",
        Some(&body),
        Some("/dashboard?inbox=1"),
        None,
        agent.as_deref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-effects-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn ctx() -> ToolContext {
        ToolContext { session_key: "agent:a1:workflow:run-9".into(), ..Default::default() }
    }

    /// The whole contract: written before the attempt; the same send
    /// again is answered from the ledger; a refusal may be retried; an
    /// unknown outcome is held and the owner told, once.
    #[tokio::test]
    async fn a_send_is_recorded_before_it_goes_and_never_goes_twice() {
        let s = store();
        let c = ctx();
        let input = serde_json::json!({"to": "+15551234567", "text": "hello"});
        let user = s.ensure_local_user_id().unwrap();

        let r = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &input, || async { SendOutcome::Sent("Sent.".into(), Some("SM123".into())) }).await;
        assert!(!r.is_error, "{}", r.content);
        let again = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &input, || async { panic!("must not send twice") }).await;
        assert!(!again.is_error);
        assert!(again.content.contains("Already sent") && again.content.contains("SM123"));
        assert_eq!(run_ref(&c), "run-9");

        // A different text is a different send.
        let other = serde_json::json!({"to": "+15551234567", "text": "hello again"});
        let r = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &other, || async { SendOutcome::ConfirmedFailure("bad number".into()) }).await;
        assert!(r.is_error);
        let r = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &other, || async { SendOutcome::Sent("Sent.".into(), None) }).await;
        assert!(!r.is_error, "a refusal may be retried");

        // Unknown: held, owner told, never retried.
        let third = serde_json::json!({"to": "+15551234567", "text": "third"});
        let r = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &third, || async { SendOutcome::Unknown("no answer".into()) }).await;
        assert!(r.is_error && r.content.contains("NOT retried"));
        let r = guarded_send(&s, &c, "messaging", "hub-sms", "sms.message.send", &third, || async { panic!("held sends are never retried") }).await;
        assert!(r.is_error && r.content.contains("outcome is unknown"));
        let key = send_key("run-9", "sms.message.send", &third);
        let id = s.engine_effect_pending("run-9", "messaging", &key, "hub-sms", "").unwrap();
        let e = s.engine_get_effect(id).unwrap().unwrap();
        assert_eq!((e.state.as_str(), e.attempts), ("pending", 1));
        assert!(s.get_notification(&format!("attention:effect:{id}"), &user).unwrap().is_some());
    }

    /// A plugin vouches for its send with a typed outcome; one that says
    /// nothing typed is treated as unknown, never guessed from words.
    #[test]
    fn plugin_outcomes_are_typed_or_unknown() {
        assert_eq!(
            SendOutcome::from_plugin_output(r#"Sent.\n{"outcome":"confirmed_success","message":"Email sent","reference":"msg-1"}"#),
            SendOutcome::Sent("Email sent".into(), Some("msg-1".into()))
        );
        assert_eq!(SendOutcome::from_plugin_output(r#"{"outcome":"confirmed_failure","message":"invalid recipient"}"#), SendOutcome::ConfirmedFailure("invalid recipient".into()));
        assert_eq!(SendOutcome::from_plugin_output(r#"{"outcome":"retryable_pre_send_failure","message":"not authenticated"}"#), SendOutcome::PreSendFailure("not authenticated".into()));
        assert!(matches!(SendOutcome::from_plugin_output(r#"{"outcome":"unknown_outcome","message":"request timed out"}"#), SendOutcome::Unknown(_)));
        assert!(matches!(SendOutcome::from_plugin_output("Error: connection refused"), SendOutcome::Unknown(_)), "words are not a contract");
        assert!(matches!(SendOutcome::from_plugin_output(r#"{"ok":true}"#), SendOutcome::Unknown(_)), "untyped JSON is not a contract either");
    }
}
