//! `ask_owner`: one question to the owner, and the wait for the answer. A
//! helper's question goes back to the conversation that started it. In
//! another run nobody is watching, it goes up the reporting line instead.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct AskOwnerTool {
    store: Arc<Store>,
    /// The coworker rail the `message` tool holds: an unanswerable question
    /// in an unattended run travels up the reporting line on it.
    coworker_rail: crate::coworker::CoworkerRailCell,
}

impl AskOwnerTool {
    pub fn new(store: Arc<Store>, coworker_rail: crate::coworker::CoworkerRailCell) -> Self {
        Self {
            store,
            coworker_rail,
        }
    }

    /// Take a question this seat cannot answer to the seat it answers to.
    ///
    /// The reporting line (`agents.reports_to`) read through the store's ONE
    /// walk, delivered on the ONE coworker rail — the manager receives it in
    /// their own session, under their own persona and memory, exactly as if
    /// a coworker had messaged them. The call returns at once; their reply
    /// comes back to this session as a notification.
    ///
    /// `None` when there is no reporting line, no rail wired, or the delivery
    /// failed: the caller then decides for itself, as every seat did before
    /// the line existed.
    async fn ask_up_the_line(&self, ctx: &ToolContext, text: &str) -> Option<ToolResult> {
        let me = types::keyparser::extract_agent_id(&ctx.session_key);
        if me.is_empty() {
            return None;
        }
        let (manager_id, _) = self.store.manager_chain(&me).ok()?.into_iter().next()?;
        let rail = self.coworker_rail.read().ok()?.clone()?;
        let my_name = self
            .store
            .get_agent(&me)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| me.clone());
        let asked = format!(
            "[{my_name} cannot finish this without a decision, and there is nobody at the \
             keyboard. You are the employee they answer to.]\n\n{text}"
        );
        match crate::coworker::deliver(&rail, ctx, &manager_id, &asked, None).await {
            Ok(delivery) => Some(ToolResult::ok(format!(
                "Nobody is at the keyboard, so this went to {name}, who you answer to. They decide \
                 in their own session, and their answer comes to you as a notification. Until \
                 then carry on with anything that doesn't depend on it, and don't treat it as \
                 decided. If a helper asked it, send_message it the answer in `answer` once it comes.",
                name = delivery.to_name
            ))
            // The same hand-off line a message to a coworker shows.
            .with_payload(serde_json::json!({
                "kind": "coworker_message",
                "to": delivery.to_name,
                "toAgentId": delivery.to_agent_id,
                "threadKey": delivery.thread_key,
                "text": text,
                "handoffId": delivery.handoff_id,
            }))),
            Err(e) => {
                tracing::warn!(agent = %me, manager = %manager_id, error = %e,
                    "escalation up the reporting line failed; the seat decides for itself");
                None
            }
        }
    }

    async fn ask(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let steps = steps(input);
        // A helper never talks with the owner: the conversation that
        // started it does. Its question ends its turn and goes back in its
        // report; that conversation asks and sends the answer on.
        if ctx.door == types::permissions::Door::Helper {
            return back_to_the_starter(input, &steps);
        }
        // Asking needs someone at the keyboard: an automated, workflow,
        // channel or helper run would wait on a card nobody sees.
        if crate::origin::ExecutionMode::from(ctx.origin)
            != crate::origin::ExecutionMode::Interactive
            || ctx.ask_channels.is_none()
        {
            // A seat that answers to another seat is not on its own: the
            // question goes up the reporting line. A seat that answers to the
            // owner still decides for itself: this is not a new way to
            // interrupt the owner.
            if let Some(answered) = self.ask_up_the_line(ctx, &escalation(input, &steps)).await {
                return answered;
            }
            return ToolResult::error(
                "Nobody is at the keyboard in this run, and you answer to the owner directly \
                 rather than to another employee — make a reasonable decision and proceed, and \
                 tell the owner what you assumed. If a helper asked it, send_message the helper \
                 your decision in `answer` so it goes on.",
            );
        }
        let Some(first) = steps.first() else {
            return ToolResult::error("question can't be empty.");
        };
        let mut widgets = Value::Array(steps.iter().map(Step::widget).collect());
        // What he is deciding on travels on the card, above the question:
        // the brief, the draft, the plan itself, not a tool row far above
        // (2026-10-19: "Want this every morning?" under a collapsed brief).
        if let Some(context) = input["context"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
            widgets[0]["context"] = json!(context);
        }
        match ctx.ask_user(&first.question, widgets).await {
            Some(response) if response == crate::origin::SKIP_SENTINEL => ToolResult::ok(
                "The owner skipped this question. Make a reasonable assumption and carry on, but \
                 tell them what you assumed. If a helper asked it, send_message the helper your decision \
                 in `answer` so it goes on.",
            ),
            Some(response) => ToolResult::ok(answers(&steps, &response).to_string()),
            None => ToolResult::error(
                "No app is connected to show the question. Make a reasonable decision and \
                 proceed, or set out the options in your reply. If a helper asked it, send_message the \
                 helper your decision in `answer` so it goes on.",
            ),
        }
    }
}

/// The most questions one card steps through.
const MAX_STEPS: usize = 4;

/// One choice on a question: a short label, and a line saying what it means.
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    label: String,
    description: String,
}

/// One question of an ask, as the card shows it: a step of its wizard. A
/// single question is a wizard of one step.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    /// The few words its step chip shows; empty on a single question.
    header: String,
    question: String,
    options: Vec<Choice>,
    multi_select: bool,
    style: String,
    /// A question Nebo itself asks: the keys it is worded by.
    words: Option<serde_json::Map<String, Value>>,
}

impl Step {
    fn from(v: &Value) -> Self {
        let text = |key: &str| v[key].as_str().map(str::trim).unwrap_or("").to_string();
        Step {
            header: text("header"),
            question: text("question"),
            options: v["options"].as_array().map(|a| a.iter().filter_map(choice).collect()).unwrap_or_default(),
            multi_select: v["multi_select"].as_bool().unwrap_or(false),
            style: text("style"),
            words: v["words"].as_object().cloned(),
        }
    }

    fn labels(&self) -> Vec<String> {
        self.options.iter().map(|o| o.label.clone()).collect()
    }

    /// The step as its card widget: an option is its label alone, or
    /// `{label, description}` when it has one.
    fn widget(&self) -> Value {
        let options: Vec<Value> = self
            .options
            .iter()
            .map(|o| if o.description.is_empty() { json!(o.label) } else { json!({"label": o.label, "description": o.description}) })
            .collect();
        let mut w = json!({ "type": "options", "multiSelect": self.multi_select, "options": options, "question": self.question });
        if !self.header.is_empty() {
            w["header"] = json!(self.header);
        }
        // A long list shows as a dropdown. The card keeps `options`, so an
        // app that doesn't know `style` still shows the choices as buttons.
        if self.style == "select" {
            w["style"] = json!("select");
        }
        // A question Nebo itself asks (the code check's) carries the message
        // keys the owner's apps word it by in his language; its English is
        // the fallback, and an option still answers with its English label.
        // An employee's own question is shown as it wrote it.
        if let Some(words) = &self.words {
            for key in WORD_FIELDS {
                if let Some(v) = words.get(key).filter(|v| !v.is_null()) {
                    w[key] = v.clone();
                }
            }
        }
        w
    }
}

/// An option as given: a label, or `{label, description}`.
fn choice(o: &Value) -> Option<Choice> {
    let (label, description) = match o {
        Value::String(s) => (s.as_str(), ""),
        Value::Object(m) => (m.get("label")?.as_str()?, m.get("description").and_then(Value::as_str).unwrap_or("")),
        _ => return None,
    };
    let label = label.trim();
    (!label.is_empty()).then(|| Choice { label: label.to_string(), description: description.trim().to_string() })
}

/// The fields of one question given at the top level.
const ONE_QUESTION: [&str; 6] = ["header", "question", "options", "multi_select", "style", "words"];

/// The call with its questions in `questions` and each option a
/// `{label}` object. A question Nebo asks itself (the code check's, with its
/// `words`) and an ask from before the wizard give one question at the top
/// level: it becomes the one entry. An option given as a plain string is
/// its label.
fn normalized(input: Value) -> Value {
    let Value::Object(mut fields) = input else { return input };
    if !fields.contains_key("questions") && fields.contains_key("question") {
        let one: serde_json::Map<String, Value> = ONE_QUESTION.iter().filter_map(|k| fields.remove(*k).map(|v| (k.to_string(), v))).collect();
        fields.insert("questions".into(), json!([one]));
    }
    if let Some(list) = fields.get_mut("questions").and_then(Value::as_array_mut) {
        for options in list.iter_mut().filter_map(|q| q.get_mut("options")).filter_map(Value::as_array_mut) {
            for o in options.iter_mut().filter(|o| o.is_string()) {
                *o = json!({ "label": o.clone() });
            }
        }
    }
    Value::Object(fields)
}

/// The ask's questions, in order.
fn steps(input: &Value) -> Vec<Step> {
    normalized(input.clone())["questions"].as_array().map(|list| list.iter().map(Step::from).collect()).unwrap_or_default()
}

/// The fields of a harness question's `words` the card carries.
const WORD_FIELDS: [&str; 5] = ["questionKey", "questionValues", "contextKey", "contextValues", "optionKeys"];

/// The question as it goes up the reporting line: each question, what is
/// decided on, the options and the recommended one, so the employee
/// answering has all the owner's card would have shown.
fn escalation(input: &Value, steps: &[Step]) -> String {
    let mut out = String::new();
    for (i, step) in steps.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        if steps.len() > 1 {
            out.push_str(&format!("{}. ", i + 1));
        }
        out.push_str(&step.question);
        if i == 0 {
            if let Some(context) = input["context"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
                out.push_str(&format!("\n\nContext: {context}"));
            }
        }
        if let Some(first) = step.options.first() {
            let pick = if step.multi_select { "any of" } else { "one of" };
            let shown: Vec<String> = step
                .options
                .iter()
                .map(|o| if o.description.is_empty() { o.label.clone() } else { format!("{} ({})", o.label, o.description) })
                .collect();
            out.push_str(&format!("\nOptions ({pick}): {}\nRecommended: {}", shown.join(" | "), first.label));
        }
    }
    out
}

/// A helper's question for the owner, as the result that ends its turn: the
/// question travels on the result ([`ToolResult::question`]) to the
/// conversation that started the helper.
fn back_to_the_starter(input: &Value, steps: &[Step]) -> ToolResult {
    let context = input["context"].as_str().map(str::trim).unwrap_or("").to_string();
    // One question travels as it is. Several travel as one question that
    // sets each out, with its choices: the conversation that started the
    // helper asks them again as its own card.
    let question = match steps {
        [one] => types::OwnerQuestion {
            question: one.question.clone(),
            context,
            options: one.labels(),
            multi_select: one.multi_select,
            style: one.style.clone(),
        },
        _ => types::OwnerQuestion { question: escalation(&json!({}), steps), context, ..Default::default() },
    };
    ToolResult {
        content: "Your question goes back with your report to the conversation that started you. It asks \
                  the owner and sends you the answer, as a message that starts \"The owner's answer to your \
                  question\"; you continue from here then. This turn ends now. The question stays open until \
                  that answer comes: any other message leaves it open, and you don't ask it again."
            .to_string(),
        terminal: true,
        question: Some(question),
        ..Default::default()
    }
}

/// How many lines of `question` are list items ("- ", "* ", "• ", "1. ",
/// "2) "): choices set out in text.
fn listed_choices(question: &str) -> usize {
    question
        .lines()
        .map(str::trim_start)
        .filter(|l| {
            ["- ", "* ", "• "].iter().any(|b| l.starts_with(b)) || {
                let digits = l.chars().take_while(char::is_ascii_digit).count();
                digits > 0 && (l[digits..].starts_with(". ") || l[digits..].starts_with(") "))
            }
        })
        .count()
}

/// What the employee is told beside the answer to an open question.
const OPEN_ANSWER_NOTE: &str = "His reply to your question. If it doesn't answer it, it is still what he said: act on it, and ask again only if you still need to know.";

/// The owner's answer as the employee reads it. On a card with options,
/// only an answer that is one of them (on a multi-select card, several
/// joined by ", ") is a pick; anything else is his own words and picks
/// nothing. Live 2026-10-02: the owner typed "k" while a choice waited, and
/// the employee told him "you picked option A".
fn answer(options: &[String], multi_select: bool, response: &str) -> Value {
    // An open question is answered by his next message in the
    // conversation, whatever it says: it may be something else he wants.
    if options.is_empty() {
        return json!({
            "response": response,
            "note": OPEN_ANSWER_NOTE,
        });
    }
    let is_option = |p: &str| options.iter().any(|o| o == p.trim());
    if is_option(response) {
        return if multi_select { json!({ "picked": [response.trim()] }) } else { json!({ "picked": response.trim() }) };
    }
    if multi_select {
        let parts: Vec<&str> = response.split(", ").map(str::trim).collect();
        if parts.iter().all(|p| is_option(p)) {
            return json!({ "picked": parts });
        }
    }
    json!({
        "own_words": response,
        "note": "He wrote this himself: it is none of the options, so nothing was picked. Take it as what he wrote; if it doesn't settle the question, ask again.",
    })
}

/// The owner's reply to the whole ask, as the employee reads it. One
/// question: its answer. Several: the card sends one answer per question,
/// in order, as a JSON list; each is read against its own question. Words
/// that are not that list (typed in the chat, or from an older app) are his
/// own words for the set.
fn answers(steps: &[Step], response: &str) -> Value {
    if let [one] = steps {
        return answer(&one.labels(), one.multi_select, response);
    }
    match serde_json::from_str::<Vec<String>>(response) {
        Ok(each) if each.len() == steps.len() => json!({
            "answers": steps
                .iter()
                .zip(&each)
                .map(|(step, said)| {
                    let mut a = if said.is_empty() {
                        json!({ "skipped": true })
                    } else {
                        answer(&step.labels(), step.multi_select, said)
                    };
                    a["question"] = json!(step.question);
                    a
                })
                .collect::<Vec<_>>(),
        }),
        _ => json!({
            "own_words": response,
            "note": "He answered all the questions at once in his own words, so nothing was picked. Take it as what he wrote; ask again only what it leaves open.",
        }),
    }
}

impl DynTool for AskOwnerTool {
    fn name(&self) -> &str {
        "ask_owner"
    }

    fn description(&self) -> String {
        "Asks the owner up to 4 questions on one card; the work waits for the answers.\n\
         - Only for real ambiguity: readings that lead to different work, or a choice only the owner can make. A clear instruction your mode allows is carried out, never asked back.\n\
         - To have him pick, give `options`: a `label` of 1 to 5 words and a one-line `description`, the recommended option first. They show where he types, on desktop and in the mobile app, with \"Other\" for his own words. Never list them in text or draw a panel. Leave them out for a free answer.\n\
         - Several decisions: one entry each in `questions`, each with a `header` of a few words.\n\
         - 6+ options: `style` \"select\".\n\
         - What he decides on (a brief, a draft, a plan) goes in `context`, shown above. The card is the one place the question appears: never write it in your message too.\n\
         - In a helper run your turn ends here: the question goes to the conversation that started you, which sends the answer back."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "maxItems": MAX_STEPS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "header": { "type": "string" },
                            "question": { "type": "string", "description": "Complete enough to answer without scrolling back." },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": { "label": { "type": "string" }, "description": { "type": "string" } },
                                    "required": ["label"]
                                }
                            },
                            "multi_select": { "type": "boolean" },
                            "style": { "type": "string", "enum": ["buttons", "select"] }
                        },
                        "required": ["question"]
                    }
                },
                "context": { "type": "string" }
            }
        })
    }

    fn normalize_input(&self, input: Value) -> Value {
        normalized(input)
    }

    fn search_hint(&self) -> &str {
        "ask the owner a question"
    }

    fn should_defer(&self) -> bool {
        false
    }

    /// Asking changes nothing, but it holds the owner's attention: never
    /// alongside other calls.
    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let steps = steps(input);
        if steps.is_empty() || steps.iter().any(|s| s.question.is_empty()) {
            return Err("question can't be empty.".to_string());
        }
        if steps.len() > MAX_STEPS {
            return Err(format!("One card holds at most {MAX_STEPS} questions. Ask the rest once these are answered."));
        }
        if steps.len() > 1 && steps.iter().any(|s| s.header.is_empty()) {
            return Err("With several questions, give each a `header` of a few words for its step.".to_string());
        }
        // Choices written into the question show as text the owner can't
        // tap (live 2026-10-08: three video lengths as a bullet list, a card
        // with only Other… and Skip). Send them back to be buttons.
        if steps.iter().any(|s| s.options.is_empty() && listed_choices(&s.question) >= 2) {
            return Err(
                "The question lists choices in its text, where the owner can't tap them. Put \
                 each choice in `options` as a short label (the recommended one first) and keep \
                 `question` to the question itself."
                    .to_string(),
            );
        }
        Ok(())
    }

    fn describe(&self, _input: &Value) -> String {
        "asking you".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.ask(&input, ctx).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unattended_run_with_no_manager_decides_for_itself() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tool = AskOwnerTool::new(store, crate::coworker::new_rail_cell());
        assert!(
            !tool.should_defer()
                && tool.read_only(&json!({}))
                && !tool.concurrency_safe(&json!({}))
        );
        let ctx = ToolContext {
            origin: crate::origin::Origin::Workflow,
            ..Default::default()
        };
        let r = tool
            .execute_dyn(&ctx, json!({"question": "Which vendor?"}))
            .await;
        assert!(
            r.is_error && r.content.contains("make a reasonable decision"),
            "{}",
            r.content
        );
        // A run that asked for its helper can't leave the helper waiting.
        assert!(r.content.contains("If a helper asked it, send_message the helper your decision in `answer` so it goes on."), "{}", r.content);
    }

    /// A helper can't reach the owner, whether or not an app could show a
    /// card and whoever its employee answers to: its question ends its turn
    /// and travels on the result, with the context and the options (the
    /// recommended one first), back to the conversation that started it.
    /// No card goes out and nothing goes up the reporting line.
    #[tokio::test]
    async fn a_helpers_question_goes_back_to_the_conversation_that_started_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tool = AskOwnerTool::new(store, crate::coworker::new_rail_cell());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let ctx = ToolContext {
            origin: crate::origin::Origin::User,
            door: types::permissions::Door::Helper,
            session_key: "subagent:agent:sdr:web:h-1".into(),
            stream_tx: Some(tx),
            ask_channels: Some(Default::default()),
            ..Default::default()
        };
        let input = json!({"question": " Which vendor? ", "context": "Two quotes.", "options": ["Acme", "Globex"], "multi_select": true, "style": "select"});
        let r = tool.execute_dyn(&ctx, input).await;
        assert!(r.terminal && !r.is_error, "it ends the turn, and is no failure: {r:?}");
        assert!(r.content.contains("goes back with your report"), "{}", r.content);
        let q = r.question.expect("the question travels on the result");
        assert_eq!((q.question.as_str(), q.context.as_str(), q.recommended()), ("Which vendor?", "Two quotes.", Some("Acme")));
        assert_eq!(q.options, ["Acme", "Globex"]);
        assert!(q.multi_select && q.style == "select", "the card's shape travels with it");
        assert!(tool.description().contains("In a helper run your turn ends here"), "{}", tool.description());
        assert!(rx.try_recv().is_err(), "no card goes out");
    }

    /// A question that goes up the reporting line carries what the card
    /// would have: the context, the options and the recommended one. A
    /// skipped question still reminds the asker that a helper may be
    /// waiting on it.
    #[tokio::test]
    async fn an_escalated_question_keeps_its_options_and_a_skip_frees_the_helper() {
        let up = |input: Value| escalation(&input, &steps(&input));
        assert_eq!(
            up(json!({"question": "Which vendor?", "context": "Two quotes.", "options": ["Acme", "Globex"]})),
            "Which vendor?\n\nContext: Two quotes.\nOptions (one of): Acme | Globex\nRecommended: Acme"
        );
        assert_eq!(up(json!({"question": "  Budget? "})), "Budget?");
        // Each question of a wizard, numbered, with what each choice means.
        assert_eq!(
            up(json!({"questions": [
                {"header": "Vendor", "question": "Which vendor?", "options": [{"label": "Acme", "description": "Cheaper, slower"}, "Globex"]},
                {"header": "Start", "question": "When to start?", "options": ["Monday"]}
            ]})),
            "1. Which vendor?\nOptions (one of): Acme (Cheaper, slower) | Globex\nRecommended: Acme\n\n2. When to start?\nOptions (one of): Monday\nRecommended: Monday"
        );
        let (_, r) = asked(json!({"question": "Which vendor?", "options": ["Acme", "Globex"]}), crate::origin::SKIP_SENTINEL).await;
        assert!(r.content.contains("If a helper asked it, send_message the helper your decision"), "{}", r.content);
    }

    /// Ask with the owner at the keyboard: the card that goes out, and the
    /// result once `reply` answers it, as the WS `ask_response` does.
    async fn asked(input: Value, reply: &str) -> (ai::StreamEvent, ToolResult) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap());
        let tool = AskOwnerTool::new(store, crate::coworker::new_rail_cell());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let channels: crate::origin::AskChannels = Default::default();
        let ctx = ToolContext {
            origin: crate::origin::Origin::User,
            stream_tx: Some(tx),
            ask_channels: Some(channels.clone()),
            ..Default::default()
        };
        let run = tokio::spawn(async move { tool.execute_dyn(&ctx, input).await });
        let card = rx.recv().await.expect("the card goes out");
        let id = card.error.clone().expect("the card has its request id");
        channels.lock().await.remove(&id).expect("the call waits on it").send(reply.to_string()).unwrap();
        (card, run.await.unwrap())
    }

    /// Live 2026-10-19: "Want this waiting for you every morning?" sat under
    /// a collapsed tool row that held the brief it meant. What the owner is
    /// deciding on travels on the card, above the question; the question
    /// stays the question.
    #[tokio::test]
    async fn what_the_owner_decides_on_travels_on_the_card() {
        let brief = "**Today**: 2 meetings; Dana is waiting on the lease.";
        let q = json!({"question": "Want this every morning?", "context": format!("  {brief}\n"), "options": ["Weekdays at 7:30", "Not now"]});
        let (card, r) = asked(q, "Not now").await;
        assert_eq!(card.text, "Want this every morning?");
        let widgets = card.widgets.expect("the card");
        assert_eq!(widgets[0]["context"], brief);
        assert!(!r.is_error, "{}", r.content);

        // No context, or only whitespace: the card carries none.
        let (card, _) = asked(json!({"question": "Which?", "context": "  ", "options": ["A", "B"]}), "A").await;
        assert!(card.widgets.expect("the card")[0].get("context").is_none());
        let schema = AskOwnerTool::new(Arc::new(Store::new(&tempfile::tempdir().unwrap().path().join("a.db").to_string_lossy()).unwrap()), crate::coworker::new_rail_cell()).schema();
        assert!(schema["properties"]["context"].is_object());
    }

    /// A question Nebo itself asks travels with the keys the owner's apps
    /// word it by in his language; its English stays the question, and his
    /// pick is still the English label. An employee's question has none.
    #[tokio::test]
    async fn a_harness_question_carries_its_words_for_the_owners_language() {
        let q = json!({
            "question": "The review of /p could not finish: 1 reviewer failed 3 times. Run it again?",
            "options": ["Keep going", "Stop here"],
            "words": {
                "questionKey": "verify.ask.reviewFailed",
                "questionValues": {"project": "/p", "count": 1, "tries": 3},
                "optionKeys": ["verify.ask.keepGoing", "verify.ask.stopHere"],
                "contextKey": null,
                "unrelated": "dropped"
            }
        });
        let (card, r) = asked(q, "Keep going").await;
        assert!(card.text.starts_with("The review of /p"), "the English is the fallback");
        let w = &card.widgets.expect("the card")[0];
        assert_eq!(w["questionKey"], "verify.ask.reviewFailed");
        assert_eq!(w["questionValues"]["tries"], 3);
        assert_eq!(w["optionKeys"], json!(["verify.ask.keepGoing", "verify.ask.stopHere"]));
        assert!(w.get("contextKey").is_none() && w.get("unrelated").is_none());
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap()["picked"], "Keep going");
        let (card, _) = asked(json!({"question": "Which?", "options": ["A", "B"]}), "A").await;
        assert!(card.widgets.expect("the card")[0].get("questionKey").is_none());
    }

    /// Live 2026-10-02: asked "can you use the cards?", Chief drew an A2UI
    /// panel nobody saw. A pick for the owner is this card: its options go
    /// out as buttons, and the one he taps comes back as his pick.
    #[tokio::test]
    async fn a_pick_goes_out_as_buttons_and_the_tapped_option_is_the_pick() {
        let q = json!({"question": "What should I focus on first?", "options": ["Invoicing", "Marketing", "A project"]});
        let (card, r) = asked(q, "Marketing").await;
        assert_eq!(card.text, "What should I focus on first?");
        let widgets = card.widgets.expect("the card has its options");
        assert_eq!(widgets[0]["type"], "options");
        assert_eq!(widgets[0]["options"], json!(["Invoicing", "Marketing", "A project"]));
        assert!(!r.is_error, "{}", r.content);
        let v: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["picked"], "Marketing", "{v}");

        // A long list asked as a dropdown: the same options, marked
        // `style: "select"`; an app that doesn't know it still has them as
        // buttons. The pick comes back the same way.
        let kits = ["Acme", "Globex", "Initech", "Umbrella", "Hooli", "Stark", "Wayne"];
        let q = json!({"question": "Which brand?", "options": kits, "style": "select"});
        let (card, r) = asked(q, "Hooli").await;
        let widgets = card.widgets.expect("the card has its options");
        assert_eq!(widgets[0]["type"], "options");
        assert_eq!(widgets[0]["style"], "select");
        assert_eq!(widgets[0]["options"], json!(kits));
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap()["picked"], "Hooli");
        // Left out, the card is exactly what it was: no `style`.
        let q = json!({"question": "Which one?", "options": ["A", "B"]});
        let (card, _) = asked(q, "A").await;
        assert!(card.widgets.unwrap()[0].get("style").is_none());

        // Several, on a multi-select card.
        let q = json!({"question": "Which days?", "options": ["Mon", "Tue", "Wed"], "multi_select": true});
        let (_, r) = asked(q, "Mon, Wed").await;
        let v: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["picked"], json!(["Mon", "Wed"]), "{v}");
    }

    /// Live 2026-10-02: the owner typed "k" while a choice waited and was
    /// told "you picked option A". Words that are none of the options pick
    /// nothing, and the employee is told so.
    #[tokio::test]
    async fn his_own_words_are_never_a_pick() {
        for words in ["k", "Marketing and invoicing"] {
            let q = json!({"question": "What should I focus on first?", "options": ["Invoicing", "Marketing", "A project"]});
            let (_, r) = asked(q, words).await;
            let v: Value = serde_json::from_str(&r.content).unwrap();
            assert!(v.get("picked").is_none(), "{words}: {v}");
            assert_eq!(v["own_words"], words);
            assert!(v["note"].as_str().unwrap().contains("nothing was picked"), "{v}");
        }
        // A free question has no options to pick from: the answer is the
        // answer. His next message answers it whatever it says, so the
        // employee is told it may be something else he wants.
        let (_, r) = asked(json!({"question": "What's the client's name?"}), "Acme").await;
        let v = serde_json::from_str::<Value>(&r.content).unwrap();
        assert_eq!(v["response"], "Acme");
        assert!(v.get("picked").is_none());
        assert!(v["note"].as_str().unwrap().contains("still what he said"), "{v}");
    }

    /// Live 2026-10-08: three video lengths written as a bullet list in the
    /// question left a card with nothing to tap. Listed choices without
    /// `options` go back to the employee; with `options`, or one list line,
    /// or a plain question, the ask goes out.
    #[test]
    fn choices_listed_in_the_question_go_back_as_options() {
        let tool = AskOwnerTool::new(
            Arc::new(Store::new(&tempfile::tempdir().unwrap().path().join("a.db").to_string_lossy()).unwrap()),
            crate::coworker::new_rail_cell(),
        );
        let listed = "How long should the video be?\n\n- **30 seconds** — tight cut\n- **45 seconds** — most clips\n- **60 seconds** — all clips";
        let err = tool.validate_input(&json!({"question": listed})).unwrap_err();
        assert!(err.contains("`options`"), "{err}");
        let numbered = "Which one?\n1. Acme\n2) Globex";
        assert!(tool.validate_input(&json!({"question": numbered})).is_err());
        assert!(tool.validate_input(&json!({"question": listed, "options": ["30 seconds", "45 seconds", "60 seconds"]})).is_ok());
        assert!(tool.validate_input(&json!({"question": "What's the client's name?"})).is_ok());
        assert!(tool.validate_input(&json!({"question": "I found one:\n- Acme\nIs that the client?"})).is_ok());
        assert!(tool.validate_input(&json!({"question": "Budget for 2026?"})).is_ok());
    }

    /// The model is told where the card shows and that it is the way to have
    /// the owner pick.
    #[test]
    fn the_description_makes_it_the_way_to_have_the_owner_pick() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AskOwnerTool::new(
            Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap()),
            crate::coworker::new_rail_cell(),
        );
        let d = tool.description();
        assert!(d.contains("To have him pick, give `options`") && d.contains("They show where he types, on desktop and in the mobile app"), "{d}");
        assert!(d.contains("Never list them in text or draw a panel"), "{d}");
        assert!(d.contains("`style` \"select\""), "{d}");
        assert_eq!(tool.schema()["properties"]["questions"]["items"]["properties"]["style"]["enum"], json!(["buttons", "select"]));
    }

    /// The owner compared cards: options that were whole sentences read as
    /// three paragraphs. An option is a short label with a one-line
    /// description; a plain string is still a label with none. Both shapes
    /// go out on the card, and the pick comes back as the label.
    #[tokio::test]
    async fn an_option_is_a_label_with_a_description_or_a_plain_label() {
        let tool = AskOwnerTool::new(
            Arc::new(Store::new(&tempfile::tempdir().unwrap().path().join("a.db").to_string_lossy()).unwrap()),
            crate::coworker::new_rail_cell(),
        );
        let input = json!({"questions": [{"question": "How long should the video be?", "options": [
            {"label": "30 seconds", "description": "A tight cut of the best clips"},
            "60 seconds",
            {"label": "  ", "description": "no label: dropped"}
        ]}]});
        assert!(tool.validate_input(&input).is_ok());
        let schema = tool.schema();
        let option = &schema["properties"]["questions"]["items"]["properties"]["options"]["items"];
        assert_eq!(option["required"], json!(["label"]));
        assert!(option["properties"]["description"].is_object());
        let (card, r) = asked(input, "30 seconds").await;
        assert_eq!(card.text, "How long should the video be?");
        let w = &card.widgets.expect("the card")[0];
        assert_eq!(w["options"], json!([{"label": "30 seconds", "description": "A tight cut of the best clips"}, "60 seconds"]));
        assert_eq!(w["question"], "How long should the video be?");
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap()["picked"], "30 seconds");
        // The question at the top level, as Nebo's own questions ask it.
        let (card, r) = asked(json!({"question": "Which one?", "options": ["A", {"label": "B", "description": "the other"}]}), "B").await;
        assert_eq!(card.widgets.expect("the card")[0]["options"], json!(["A", {"label": "B", "description": "the other"}]));
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap()["picked"], "B");
        // The registry runs the call as `questions`, so a question at the top
        // level is never refused as parameters the schema doesn't name.
        let one = tool.normalize_input(json!({"question": "Which?", "options": ["A"], "words": {"questionKey": "k"}, "context": "c"}));
        assert_eq!(one, json!({"context": "c", "questions": [{"question": "Which?", "options": [{"label": "A"}], "words": {"questionKey": "k"}}]}));
        assert_eq!(tool.normalize_input(one.clone()), one, "idempotent");
        assert!(crate::input_schema::unknown_parameters(&tool.schema(), &json!({"context": "c"})).is_empty());
        let d = tool.description();
        assert!(d.contains("a `label` of 1 to 5 words") && d.contains("the recommended option first") && d.contains("\"Other\""), "{d}");
    }

    /// Several decisions at once are one card the owner steps through: one
    /// widget per question, each with its header, and one reply holding
    /// every answer, read against its own question.
    #[tokio::test]
    async fn several_questions_are_one_card_and_one_reply() {
        let input = json!({"context": "The cut is ready.", "questions": [
            {"header": "Length", "question": "How long?", "options": [{"label": "30 seconds", "description": "Tight"}, "60 seconds"]},
            {"header": "Music", "question": "Which music?", "options": ["Upbeat", "Calm", "None"], "multi_select": true},
            {"header": "Title", "question": "What title?"}
        ]});
        let reply = json!(["60 seconds", "Upbeat, Calm", "Spring launch"]).to_string();
        let (card, r) = asked(input.clone(), &reply).await;
        assert_eq!(card.text, "How long?", "an app that shows one question shows the first");
        let widgets = card.widgets.expect("the card");
        let widgets = widgets.as_array().unwrap();
        assert_eq!(widgets.len(), 3);
        assert_eq!(widgets.iter().map(|w| w["header"].as_str().unwrap()).collect::<Vec<_>>(), ["Length", "Music", "Title"]);
        assert_eq!(widgets[0]["context"], "The cut is ready.");
        assert!(widgets[1].get("context").is_none());
        assert_eq!(widgets[1]["multiSelect"], true);
        let v: Value = serde_json::from_str(&r.content).unwrap();
        let a = v["answers"].as_array().expect("one answer per question");
        assert_eq!(a[0]["picked"], "60 seconds");
        assert_eq!(a[0]["question"], "How long?");
        assert_eq!(a[1]["picked"], json!(["Upbeat", "Calm"]));
        assert_eq!(a[2]["response"], "Spring launch");
        // A question he left blank is skipped; words that are not one answer
        // per question are his own words for the set.
        let (_, r) = asked(input.clone(), &json!(["30 seconds", "", "X"]).to_string()).await;
        assert_eq!(serde_json::from_str::<Value>(&r.content).unwrap()["answers"][1]["skipped"], true);
        let (_, r) = asked(input, "short, calm, no title").await;
        let v: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(v["own_words"], "short, calm, no title");
        assert!(v.get("answers").is_none());
    }

    /// A wizard's steps need their headers, and one card holds at most four.
    #[test]
    fn a_wizard_needs_headers_and_holds_four() {
        let tool = AskOwnerTool::new(
            Arc::new(Store::new(&tempfile::tempdir().unwrap().path().join("a.db").to_string_lossy()).unwrap()),
            crate::coworker::new_rail_cell(),
        );
        let q = |header: &str| json!({"header": header, "question": "Which?", "options": ["A", "B"]});
        assert!(tool.validate_input(&json!({"questions": [q("One"), q("Two")]})).is_ok());
        assert!(tool.validate_input(&json!({"questions": [q(""), q("Two")]})).unwrap_err().contains("`header`"));
        assert!(tool.validate_input(&json!({"questions": [q("")]})).is_ok(), "one question needs no header");
        assert!(tool.validate_input(&json!({"questions": [q("1"), q("2"), q("3"), q("4"), q("5")]})).unwrap_err().contains("at most 4"));
        assert!(tool.validate_input(&json!({"questions": []})).is_err());
        assert!(tool.validate_input(&json!({"questions": [{"header": "A", "question": " "}]})).is_err());
    }

    /// A helper's several questions go back as one question that sets each
    /// out with its choices: the conversation that started it asks them.
    #[tokio::test]
    async fn a_helpers_several_questions_go_back_set_out() {
        let dir = tempfile::tempdir().unwrap();
        let tool = AskOwnerTool::new(Arc::new(Store::new(&dir.path().join("a.db").to_string_lossy()).unwrap()), crate::coworker::new_rail_cell());
        let ctx = ToolContext { door: types::permissions::Door::Helper, ..Default::default() };
        let r = tool
            .execute_dyn(&ctx, json!({"questions": [
                {"header": "Vendor", "question": "Which vendor?", "options": ["Acme", "Globex"]},
                {"header": "Start", "question": "When?"}
            ]}))
            .await;
        let q = r.question.expect("the question travels on the result");
        assert_eq!(q.question, "1. Which vendor?\nOptions (one of): Acme | Globex\nRecommended: Acme\n\n2. When?");
        assert!(q.options.is_empty());
    }
}
