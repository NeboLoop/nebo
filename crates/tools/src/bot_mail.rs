//! The bot's own hosted mail address as a provider of `mail.message.send`.
//!
//! A bot paired with a hosted service can have an address of its own
//! (`nanna-7kq@nebo.bot`). Sending from it is not a second mail tool: it is
//! one more provider of the one operation, `mail_message_send`, next to any
//! connected mail plugin (the business's own mailbox). The provider exists
//! only while the address does — the server registers it after the hub
//! confirms the address and removes it when the pairing goes — so a Nebo
//! with no hosted service has exactly the tools it had before.
//!
//! Who is writing is never the model's to say: the employee whose run makes
//! the call signs it (its `+tag` brings replies back to it) and the
//! conversation it is made from rides along, so an answer comes back there.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::effects::SendOutcome;
use crate::operation_tools::{OperationProvider, ProvidedOperation};
use crate::origin::ToolContext;
use crate::registry::ToolResult;
use db::Store;

/// The provider's name, the `provider` value when several bind the send.
pub const PROVIDER: &str = "bot-address";

/// The one operation it performs.
const OPERATION: &str = "mail.message.send";

/// What one message may carry. The hub holds every message to the same
/// caps; checking here first means nothing is uploaded for a message the
/// hub would refuse. 5 MB of files is about 6.7 MB once base64 adds a
/// third: under Outlet's 8 MB body limit with room for the JSON envelope.
/// Raise these with the hub's when Outlet's route limit and the SES raw
/// send are raised. A file is never larger than the whole message.
const MAX_ATTACHMENTS: usize = 10;
const MAX_ATTACHMENT_BYTES: u64 = 5 << 20;
const MAX_ATTACHMENTS_BYTES: u64 = 5 << 20;

/// A file the call attaches, checked and ready to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

/// The files a call attaches (`attachments`: paths; `~` works), each
/// checked before anything is uploaded: it exists, it is a file, it is on
/// this Mac (not an iCloud placeholder, `file_tool::ensure_local`), it is
/// not empty, and the count, each size
/// and the total are within the caps. The first file that fails names
/// itself and the reason, and the message is not sent.
pub fn attachment_files(input: &Value) -> Result<Vec<MailFile>, String> {
    let paths: Vec<String> = match input.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(p)) => vec![p.clone()],
        Some(Value::Array(items)) => items.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
        Some(_) => return Err("`attachments` is a list of file paths.".into()),
    };
    let paths: Vec<String> = paths.into_iter().map(|p| p.trim().to_string()).collect();
    if paths.len() > MAX_ATTACHMENTS {
        return Err(format!("Not sent: {} attachments; one email can carry at most {MAX_ATTACHMENTS}.", paths.len()));
    }
    let mut files = Vec::with_capacity(paths.len());
    let mut total = 0u64;
    for raw in paths {
        let fail = |why: String| Err(format!("Not sent: {raw}: {why}. Nothing was sent; fix that file or leave it out, then send again."));
        if raw.is_empty() {
            return Err("Not sent: an attachment path is empty. Nothing was sent.".into());
        }
        let path = match types::pathres::resolve(&raw) {
            Ok(p) => p,
            Err(e) => return fail(e.to_string()),
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => return fail(format!("cannot read it ({e})")),
        };
        if !meta.is_file() {
            return fail("it is not a file".into());
        }
        if let Err(why) = crate::file_tool::ensure_local(&path.to_string_lossy()) {
            return fail(why.trim_end_matches('.').to_string());
        }
        if meta.len() == 0 {
            return fail("the file is empty (0 bytes)".into());
        }
        if meta.len() > MAX_ATTACHMENT_BYTES {
            return fail(format!(
                "the file is {}; one attachment can be at most {}",
                megabytes(meta.len()),
                megabytes(MAX_ATTACHMENT_BYTES)
            ));
        }
        total += meta.len();
        if total > MAX_ATTACHMENTS_BYTES {
            return fail(format!(
                "with it the attachments come to {}; one email can carry at most {}",
                megabytes(total),
                megabytes(MAX_ATTACHMENTS_BYTES)
            ));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| raw.clone());
        files.push(MailFile { path, name, size: meta.len() });
    }
    Ok(files)
}

/// Rounded up, so a size over a cap never reads as the cap.
fn megabytes(n: u64) -> String {
    format!("{} MB", ((n as f64) * 10.0 / (1u64 << 20) as f64).ceil() / 10.0)
}

/// The reason a hub's 4xx gives (`{"error": "..."}`), else its body.
fn hub_reason(body: String) -> String {
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(body)
}

fn names(files: &[MailFile]) -> String {
    files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The bot's own address, sending through the hub that hosts it.
pub struct BotMailProvider {
    store: Arc<Store>,
    api_url: String,
    address: String,
}

impl BotMailProvider {
    pub fn new(store: Arc<Store>, api_url: String, address: String) -> Self {
        Self { store, api_url, address }
    }

    /// The employee making the call: its tag and name, or none for the
    /// primary (the bot's own name signs its mail).
    fn signer(&self, ctx: &ToolContext) -> (String, String) {
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        if agent_id.is_empty() || agent_id == crate::team_tool::PRIMARY_AGENT_ID {
            return (String::new(), String::new());
        }
        match self.store.get_agent(&agent_id) {
            Ok(Some(a)) if !a.name.trim().is_empty() => (comm::handle::slugify(&a.name), a.name.trim().to_string()),
            _ => (String::new(), String::new()),
        }
    }

    /// The request the hub is sent for this call.
    pub fn request(&self, ctx: &ToolContext, input: &Value) -> Result<comm::api_types::BotEmailSend, String> {
        let text = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::trim).unwrap_or("").to_string();
        let to_owner = input.get("toOwner").and_then(|v| v.as_bool()).unwrap_or(false);
        let to = match input.get("to") {
            Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str()).map(str::trim).collect::<Vec<_>>().join(","),
            _ => text("to"),
        };
        if !to_owner && to.is_empty() {
            return Err("Say who it goes to: `to` (an address), or `toOwner: true` to write to the owner.".into());
        }
        if !to_owner && to.contains(',') {
            return Err("One recipient per message from the bot's own address: send one call per person.".into());
        }
        let body = text("text");
        if body.is_empty() {
            return Err("`text` is the message and it is empty; write the message in `text`.".into());
        }
        let (employee, employee_name) = self.signer(ctx);
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        let chat_id = if ctx.session_id.is_empty() {
            String::new()
        } else {
            self.store.resolve_session_chat_id(&ctx.session_id)
        };
        Ok(comm::api_types::BotEmailSend {
            to: if to_owner { String::new() } else { to },
            to_owner,
            subject: text("subject"),
            body_text: body,
            body_html: text("html"),
            employee,
            employee_name,
            inbound_email_id: String::new(),
            agent_id,
            chat_id,
            attachments: Vec::new(),
        })
    }

    async fn send(&self, mut req: comm::api_types::BotEmailSend, files: Vec<MailFile>) -> SendOutcome {
        let Some(bot_id) = config::read_bot_id() else {
            return SendOutcome::PreSendFailure("This Nebo has no bot id; the hosted address cannot send.".into());
        };
        let Some(token) = auth::neboai_token(&self.store) else {
            return SendOutcome::PreSendFailure("This Nebo is no longer paired; the hosted address cannot send.".into());
        };
        let api = comm::api::NeboAIApi::new(self.api_url.clone(), bot_id, token);
        // Every file is uploaded through the one upload path before the
        // message is sent; one that fails stops the message.
        for f in &files {
            let data = match tokio::fs::read(&f.path).await {
                Ok(d) if !d.is_empty() => d,
                Ok(_) => return SendOutcome::PreSendFailure(format!("Not sent: {}: the file is empty (0 bytes).", f.name)),
                Err(e) => return SendOutcome::PreSendFailure(format!("Not sent: {}: cannot read it ({e}).", f.name)),
            };
            match api.upload_file(&f.name, crate::loop_tool::mime_for_path(&f.path), data, &[]).await {
                Ok(a) if !a.file_id.is_empty() => req.attachments.push(a.file_id),
                Ok(_) => return SendOutcome::PreSendFailure(format!("Not sent: uploading {} returned no file id.", f.name)),
                Err(comm::CommError::Http { status, body }) if (400..500).contains(&status) => {
                    return SendOutcome::PreSendFailure(format!("Not sent: {}: {}", f.name, hub_reason(body)))
                }
                Err(e) => return SendOutcome::PreSendFailure(format!("Not sent: uploading {} failed: {e}", f.name)),
            }
        }
        let result = api.send_bot_email(&req).await;
        self.outcome(&req, &files, result)
    }

    /// The address a request goes out from: an employee's own `+tag` form
    /// of the bot's address, or the bot's address for the primary.
    fn from_address(&self, req: &comm::api_types::BotEmailSend) -> String {
        comm::handle::employee_email_address(&self.address, &req.employee_name).unwrap_or_else(|| self.address.clone())
    }

    /// What the employee is told about a send. The From it names is the one
    /// the hub reports it sent from (an employee's `+tag` address, not the
    /// bot's), so the employee never tells anyone a wrong address to answer.
    ///
    /// Files are reported attached only when the hub confirms that many
    /// went out: a hub that ignores the field sent the mail without them,
    /// and that is a failure, never a success.
    fn outcome(&self, req: &comm::api_types::BotEmailSend, files: &[MailFile], result: Result<Value, comm::CommError>) -> SendOutcome {
        let from = self.from_address(req);
        let to = if req.to_owner { "the owner".to_string() } else { req.to.clone() };
        match result {
            Ok(v) => {
                let reference = v["messageId"].as_str().map(str::to_string);
                let sent_from = v["sentFrom"].as_str().filter(|s| !s.is_empty()).unwrap_or(&from);
                if req.attachments.is_empty() {
                    return SendOutcome::Sent(format!("Sent from {sent_from} to {to}."), reference);
                }
                if v["attachments"].as_u64() != Some(req.attachments.len() as u64) {
                    return SendOutcome::ConfirmedFailure(format!(
                        "Email sent without the attachment: {}. The message went from {sent_from} to {to}, but the hub did \
                         not confirm the files went with it (it may not support attachments yet). Do not say they were \
                         attached and do not send the message again; tell the owner the files did not go.",
                        names(files)
                    ));
                }
                SendOutcome::Sent(format!("Sent from {sent_from} to {to} with {} attached.", names(files)), reference)
            }
            Err(e @ comm::CommError::Paused) => SendOutcome::PreSendFailure(e.to_string()),
            Err(comm::CommError::Http { status, body }) if (400..500).contains(&status) => {
                SendOutcome::ConfirmedFailure(format!("Not sent from {from}: {}", hub_reason(body)))
            }
            // A server error or no answer: the mail may already be on its way.
            Err(e) => SendOutcome::Unknown(format!("sending from {from}: {e}")),
        }
    }
}

impl OperationProvider for BotMailProvider {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn service(&self) -> String {
        format!("this bot's own address {}", self.address)
    }

    fn operations(&self) -> Vec<ProvidedOperation> {
        let mut properties = Map::new();
        properties.insert("to".into(), json!({"type": "string", "description": "The recipient's address."}));
        properties.insert(
            "toOwner".into(),
            json!({"type": "boolean", "description": "Write to the owner (their account email); `to` is not needed."}),
        );
        properties.insert("subject".into(), json!({"type": "string", "description": "The subject line."}));
        properties.insert("text".into(), json!({"type": "string", "description": "The message, plain text."}));
        properties.insert("html".into(), json!({"type": "string", "description": "Optional HTML version of the message."}));
        properties.insert(
            "attachments".into(),
            json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "Files to attach: full paths on this computer (~ works). At most 10 files, 5 MB together. All go or the message is not sent."
            }),
        );
        vec![ProvidedOperation {
            operation: OPERATION.to_string(),
            properties,
            required: vec!["subject".into(), "text".into()],
            note: format!(
                "{PROVIDER} sends from your own address, the Email your environment names ({} for the primary \
                 employee, its +tag form for any other), signed by you; replies come back to you. Mail to the owner \
                 is never limited; mail to anyone else counts against the bot's daily limit. To send files, list \
                 their paths in `attachments`; say a file is attached only when the result says so.",
                self.address
            ),
            ..Default::default()
        }]
    }

    fn perform<'a>(
        &'a self,
        ctx: &'a ToolContext,
        operation: &'a str,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            if operation != OPERATION {
                return ToolResult::error(format!("{PROVIDER} performs {OPERATION} only."));
            }
            let req = match self.request(ctx, &input) {
                Ok(r) => r,
                Err(why) => return ToolResult::error(why),
            };
            let files = match attachment_files(&input) {
                Ok(f) => f,
                Err(why) => return ToolResult::error(why),
            };
            crate::effects::guarded_send(&self.store, ctx, "messaging", PROVIDER, OPERATION, &input, None, || self.send(req, files)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<Store> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        std::mem::forget(dir);
        Arc::new(Store::new(&path.to_string_lossy()).unwrap())
    }

    fn provider() -> BotMailProvider {
        BotMailProvider::new(store(), "http://127.0.0.1:9".into(), "nanna-7kq@nebo.bot".into())
    }

    /// Mail to the owner names no address: the hub fills the owner's.
    #[test]
    fn mail_to_the_owner_asks_the_hub_to_address_it() {
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        let req = provider()
            .request(&ctx, &json!({"toOwner": true, "subject": "Done", "text": "The report is ready."}))
            .unwrap();
        assert!(req.to_owner);
        assert!(req.to.is_empty());
        assert!(req.employee.is_empty(), "the primary signs with the bot's own name, no tag");
        let wire = serde_json::to_value(&req).unwrap();
        assert_eq!(wire["toOwner"], true);
        assert!(wire.get("to").is_none());
        assert_eq!(wire["bodyText"], "The report is ready.");
    }

    /// An employee's mail carries its tag and name, taken from the run —
    /// never from the call.
    #[test]
    fn an_employees_mail_is_signed_with_its_tag_and_name() {
        let p = provider();
        p.store
            .create_agent("recep-1", None, "Front Desk", "", "", "---\nname: Front Desk\n---\n", None, None)
            .unwrap();
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:recep-1:web", "");
        let req = p
            .request(&ctx, &json!({"to": "pat@example.com", "subject": "Hi", "text": "Hello", "employee": "someone-else"}))
            .unwrap();
        assert_eq!(req.employee, "front-desk");
        assert_eq!(req.employee_name, "Front Desk");
        assert_eq!(req.agent_id, "recep-1");
        assert_eq!(req.to, "pat@example.com");
    }

    /// The employee is told the address its mail really went out from: its
    /// own `+tag` address, as the hub reports it, never the bot's. (Live
    /// 2026-09-28: Front Desk's mail left from `…+front-desk@nebo.bot` and it
    /// was told "Sent from …@nebo.bot", the address that reaches the primary.)
    #[test]
    fn a_send_names_the_address_it_really_went_from() {
        let p = provider();
        p.store
            .create_agent("recep-1", None, "Front Desk", "", "", "---\nname: Front Desk\n---\n", None, None)
            .unwrap();
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:recep-1:web", "");
        let req = p.request(&ctx, &json!({"to": "pat@example.com", "subject": "Hi", "text": "Hello"})).unwrap();
        let said = |o: SendOutcome| match o {
            SendOutcome::Sent(m, _) | SendOutcome::ConfirmedFailure(m) | SendOutcome::PreSendFailure(m) | SendOutcome::Unknown(m) => m,
        };

        let hub = json!({"ok": true, "messageId": "m-1@nebo.bot", "sentFrom": "nanna-7kq+front-desk@nebo.bot"});
        assert_eq!(said(p.outcome(&req, &[], Ok(hub))), "Sent from nanna-7kq+front-desk@nebo.bot to pat@example.com.");
        assert_eq!(
            said(p.outcome(&req, &[], Ok(json!({"ok": true})))),
            "Sent from nanna-7kq+front-desk@nebo.bot to pat@example.com.",
            "a hub that names no sender: the employee's own address"
        );
        let off = comm::CommError::Http { status: 403, body: r#"{"error":"email sending is turned off for this bot"}"#.into() };
        assert_eq!(
            said(p.outcome(&req, &[], Err(off))),
            "Not sent from nanna-7kq+front-desk@nebo.bot: email sending is turned off for this bot"
        );

        let primary = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        let req = p.request(&primary, &json!({"toOwner": true, "subject": "Done", "text": "Ready."})).unwrap();
        assert_eq!(said(p.outcome(&req, &[], Ok(json!({"ok": true})))), "Sent from nanna-7kq@nebo.bot to the owner.");
    }

    fn file(dir: &std::path::Path, name: &str, size: u64) -> String {
        let path = dir.join(name);
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(size).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn said(o: SendOutcome) -> (bool, String) {
        match o {
            SendOutcome::Sent(m, _) => (true, m),
            SendOutcome::ConfirmedFailure(m) | SendOutcome::PreSendFailure(m) | SendOutcome::Unknown(m) => (false, m),
        }
    }

    /// No attachments, one, several: each is found, named and sized, and
    /// a call without any asks the hub for exactly what it did before.
    #[test]
    fn attachments_are_checked_and_named() {
        let dir = tempfile::tempdir().unwrap();
        assert!(attachment_files(&json!({"text": "hi"})).unwrap().is_empty());
        assert!(attachment_files(&json!({"attachments": []})).unwrap().is_empty());
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        let wire = serde_json::to_value(provider().request(&ctx, &json!({"toOwner": true, "subject": "s", "text": "t"})).unwrap()).unwrap();
        assert!(wire.get("attachments").is_none(), "a send without files carries no attachments field");

        let png = file(dir.path(), "chart.png", 1200);
        let one = attachment_files(&json!({"attachments": png})).unwrap();
        assert_eq!((one[0].name.as_str(), one[0].size), ("chart.png", 1200));

        let several = attachment_files(&json!({"attachments": [
            png,
            file(dir.path(), "Q3 report.pdf", (5 << 20) - 1200 - 5),
            file(dir.path(), "notes.txt", 5),
        ]}))
        .unwrap();
        assert_eq!(several.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["chart.png", "Q3 report.pdf", "notes.txt"]);
        assert_eq!(several.iter().map(|f| f.size).sum::<u64>(), 5 << 20, "exactly at the total cap");
        let whole = attachment_files(&json!({"attachments": [file(dir.path(), "deck.pdf", 5 << 20)]})).unwrap();
        assert_eq!(whole[0].size, 5 << 20, "one file may fill the whole message");
    }

    /// A missing file, an empty one, or a message over a cap is refused
    /// before anything is uploaded, naming the file and why.
    #[test]
    fn a_bad_attachment_is_refused_before_anything_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        let ok = file(dir.path(), "ok.txt", 10);
        let missing = dir.path().join("gone.png").to_string_lossy().into_owned();
        let refused = |input: Value| attachment_files(&input).unwrap_err();

        let why = refused(json!({"attachments": [ok, missing]}));
        assert!(why.contains("gone.png") && why.contains("not found"), "{why}");
        let why = refused(json!({"attachments": [file(dir.path(), "empty.pdf", 0)]}));
        assert!(why.contains("empty.pdf") && why.contains("empty"), "{why}");
        let why = refused(json!({"attachments": [file(dir.path(), "huge.zip", (5 << 20) + 1)]}));
        assert!(why.contains("huge.zip: the file is 5.1 MB; one attachment can be at most 5 MB."), "{why}");
        let big = [file(dir.path(), "b0.pdf", 3 << 20), file(dir.path(), "b1.pdf", 2 << 20), file(dir.path(), "b2.pdf", 1)];
        let why = refused(json!({"attachments": big}));
        assert!(why.contains("b2.pdf: with it the attachments come to 5.1 MB; one email can carry at most 5 MB."), "{why}");
        let many: Vec<String> = (0..11).map(|_| ok.clone()).collect();
        assert!(refused(json!({"attachments": many})).contains("at most 10"));
        assert!(refused(json!({"attachments": [dir.path().to_string_lossy()]})).contains("not a file"));
    }

    /// The whole call: a missing or empty attachment stops it before the
    /// send is attempted (no hub is reached; this Nebo has none).
    #[tokio::test]
    async fn the_call_fails_on_a_bad_attachment_without_sending() {
        let dir = tempfile::tempdir().unwrap();
        let p = provider();
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        for (path, why) in [(dir.path().join("gone.png").to_string_lossy().into_owned(), "not found"), (file(dir.path(), "empty.png", 0), "empty")] {
            let input = json!({"toOwner": true, "subject": "Chart", "text": "Attached.", "attachments": [path]});
            let r = p.perform(&ctx, OPERATION, input).await;
            assert!(r.is_error && r.content.starts_with("Not sent:") && r.content.contains(why), "{}", r.content);
        }
    }

    /// Files are reported attached only when the hub says that many went.
    /// An older hub ignores the field and sends the mail without them: that
    /// is a failure, said plainly, never a success.
    #[test]
    fn attached_only_when_the_hub_confirms_the_count() {
        let p = provider();
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        let mut req = p.request(&ctx, &json!({"toOwner": true, "subject": "Chart", "text": "Here."})).unwrap();
        req.attachments = vec!["f-1".into(), "f-2".into()];
        let files = [
            MailFile { path: "/x/chart.png".into(), name: "chart.png".into(), size: 1 },
            MailFile { path: "/x/notes.txt".into(), name: "notes.txt".into(), size: 1 },
        ];

        let (ok, msg) = said(p.outcome(&req, &files, Ok(json!({"ok": true, "messageId": "m-1"}))));
        assert!(!ok && msg.starts_with("Email sent without the attachment: chart.png, notes.txt."), "{msg}");
        let (ok, msg) = said(p.outcome(&req, &files, Ok(json!({"ok": true, "attachments": 1}))));
        assert!(!ok && msg.starts_with("Email sent without the attachment"), "a short count is not a send: {msg}");
        let (ok, msg) = said(p.outcome(&req, &files, Ok(json!({"ok": true, "attachments": 2}))));
        assert!(ok, "{msg}");
        assert_eq!(msg, "Sent from nanna-7kq@nebo.bot to the owner with chart.png, notes.txt attached.");

        let refused = comm::CommError::Http { status: 400, body: r#"{"error":"chart.png: the file is empty"}"#.into() };
        let (ok, msg) = said(p.outcome(&req, &files, Err(refused)));
        assert!(!ok && msg.ends_with("chart.png: the file is empty"), "{msg}");
        assert_eq!(serde_json::to_value(&req).unwrap()["attachments"], json!(["f-1", "f-2"]));
    }

    /// A send with nobody to send to, or nothing to say, is refused before
    /// anything leaves.
    #[test]
    fn a_send_needs_a_recipient_and_a_message() {
        let ctx = ToolContext::new(crate::Origin::User);
        let p = provider();
        assert!(p.request(&ctx, &json!({"subject": "x", "text": "y"})).is_err());
        assert!(p.request(&ctx, &json!({"to": "a@example.com", "subject": "x", "text": ""})).is_err());
        assert!(p.request(&ctx, &json!({"to": ["a@example.com", "b@example.com"], "subject": "x", "text": "y"})).is_err());
    }
}
