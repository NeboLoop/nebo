//! Helpers: `delegate` starts one, `send_message` talks to a running or
//! finished one — or, by the kind of `to`, to a coworker or a team.
//! Behaviour stays in the helper registry; these are its interface.
//! Several independent pieces of work are several `delegate` calls in one
//! response. Reading a helper's output and stopping it are `read_output`
//! and `stop_task`, shared with background commands (`command_tools`).

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::coworker::CoworkerRailCell;
use crate::orchestrator::{FollowUp, OrchestratorHandle, REPORT_LIMIT, SpawnRequest, SubAgentOrchestrator};
use crate::team_tool::Teams;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// The helper types a delegate call may name.
const HELPER_TYPES: [&str; 3] = ["general", "explore", "plan"];

/// What a helper tool needs. `send_message` also reaches coworkers (the
/// coworker rail) and teams (the one [`Teams`] core).
pub struct Helpers {
    store: Arc<Store>,
    orchestrator: OrchestratorHandle,
    teams: Arc<Teams>,
    rail: CoworkerRailCell,
}

/// Who a `send_message` goes to, by the kind of `to`.
enum Recipient {
    Team(String),
    Coworker(String),
    Helper,
}

impl Helpers {
    pub fn new(store: Arc<Store>, orchestrator: OrchestratorHandle, teams: Arc<Teams>, rail: CoworkerRailCell) -> Self {
        Self {
            store,
            orchestrator,
            teams,
            rail,
        }
    }

    /// A team on this Nebo (by name or id), else an installed employee (by
    /// name, handle or id), else a helper's id.
    fn recipient(&self, to: &str) -> Recipient {
        if let Ok(team) = crate::team::resolve_team(&self.store, to) {
            return Recipient::Team(team.name);
        }
        match crate::team::resolve_agent(&self.store, to) {
            Some(a) => Recipient::Coworker(a.name),
            None => Recipient::Helper,
        }
    }

    /// Who a `send_message` goes to, as the owner reads it.
    fn recipient_label(&self, input: &Value) -> String {
        match self.recipient(input["to"].as_str().unwrap_or("").trim()) {
            Recipient::Team(name) => format!("the {name} team"),
            Recipient::Coworker(name) => name,
            Recipient::Helper => "a helper".to_string(),
        }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let helpers = Arc::new(self);
        HelperOp::ALL
            .into_iter()
            .map(|op| {
                Box::new(HelperTool {
                    op,
                    helpers: helpers.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    fn orchestrator(&self) -> Result<&dyn SubAgentOrchestrator, &'static str> {
        self.orchestrator.get().map(|o| o.as_ref()).ok_or(
            "Helpers aren't ready yet: the server is still starting. Try again in a moment, \
             or do the work yourself.",
        )
    }

    /// A helper has no name: a call that hands its work to an employee is
    /// work for a coworker, and an anonymous helper would impersonate them
    /// (smoke, 2026-09-05: "Chief of Staff" got a blank helper).
    fn names_an_employee(&self, input: &Value) -> Option<String> {
        let names: Vec<String> = self
            .store
            .list_agents(500, 0)
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.name)
            .collect();
        let who = employee_addressed(
            input["prompt"].as_str().unwrap_or(""),
            input["helper_type"].as_str().unwrap_or(""),
            &names,
        )?;
        Some(format!(
            "A helper is an anonymous extra pair of hands; \"{who}\" is an employee. Work for an \
             employee is a message: send_message(to: \"{who}\", message: \"<what you need>\"). \
             Not started."
        ))
    }

    async fn delegate(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        if let Some(refusal) = self.names_an_employee(input) {
            return ToolResult::error(refusal);
        }
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        let background = input["background"].as_bool().unwrap_or(true);
        let req = SpawnRequest {
            prompt: input["prompt"].as_str().unwrap_or("").to_string(),
            description: input["description"].as_str().unwrap_or("").to_string(),
            agent_type: input["helper_type"]
                .as_str()
                .unwrap_or("general")
                .to_string(),
            wait: !background,
            // An isolated helper works in its own copy of the project, merged
            // back when it finishes; the report says how the merge went.
            isolate: if input["isolation"].as_str() == Some("worktree") {
                "worktree".to_string()
            } else {
                String::new()
            },
            speed: input["speed"].as_str().unwrap_or("").trim().to_string(),
            report_limit: report_limit(input),
            ..SpawnRequest::child_of(ctx)
        };
        if let Some(pieces) = input["pieces"].as_array() {
            return fan_out(orch, req, pieces).await;
        }
        // The harness words what happened: the launch receipt, or the
        // finished helper's report with how to continue it.
        match orch.spawn(req).await {
            Ok(r) if r.success => ToolResult::ok(r.output).with_taint(r.taint),
            Ok(r) => ToolResult::error(r.output).with_taint(r.taint),
            Err(e) => ToolResult::error(format!("Couldn't start the helper: {e}")),
        }
    }

    async fn send_message(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let to = input["to"].as_str().unwrap_or("").trim();
        let message = input["message"].as_str().unwrap_or("").trim();
        if let Some(changes) = input["changes"].as_str() {
            return self.settle_changes(to, changes, ctx).await;
        }
        let conversation = input["conversation"].as_str().and_then(crate::coworker::Conversation::parse);
        let answer = input["answer"].as_str().map(str::trim).filter(|a| !a.is_empty()).map(|text| crate::orchestrator::Answer {
            question: input["question"].as_str().map(str::trim).filter(|q| !q.is_empty()).map(str::to_string),
            text: text.to_string(),
        });
        let recipient = self.recipient(to);
        if answer.is_none() && input["question"].as_str().is_some_and(|q| !q.trim().is_empty()) {
            return ToolResult::error(
                "`question` names the helper's question an `answer` settles, and no `answer` was given. Nothing was \
                 sent. Send it again with the owner's decision in `answer`, or without `question`.",
            );
        }
        if answer.is_some() && !matches!(recipient, Recipient::Helper) {
            return ToolResult::error(
                "`answer` carries the owner's answer to a helper's question; a coworker or a team asked none. \
                 Nothing was sent. Send it again without `answer`.",
            );
        }
        if conversation.is_some() && !matches!(recipient, Recipient::Coworker(_)) {
            return ToolResult::error(
                "`conversation` picks one of a linked employee's conversations; a team or a helper has one \
                 conversation with you. Nothing was sent. Send it again without `conversation`.",
            );
        }
        match recipient {
            Recipient::Team(team) => return self.teams.post(ctx, &team, message, &input["mention"]).await,
            Recipient::Coworker(name) => return self.to_coworker(ctx, &name, message, conversation).await,
            Recipient::Helper => {}
        }
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        match orch
            .send(to, message, answer, SpawnRequest::child_of(ctx))
            .await
        {
            Ok(FollowUp::Delivered { task_id }) => ToolResult::ok(format!(
                "Delivered to helper [{task_id}] while it works; it sees your message at its next \
                 step. Its result reaches you the way it was started to report — until then you \
                 know nothing about how it went."
            )),
            Ok(FollowUp::Continued(r)) if r.success => ToolResult::ok(format!(
                "Helper [{id}] continued:\n\n{out}\n\nsend_message to {id} again to refine it further.",
                id = r.task_id,
                out = r.output
            )),
            Ok(FollowUp::Continued(r)) => ToolResult::error(format!(
                "Helper [{}] failed: {}",
                r.task_id,
                r.error.unwrap_or_default()
            )),
            Err(e) => ToolResult::error(e),
        }
    }
}

impl Helpers {
    /// Land or discard an isolated helper's changes (the helper door
    /// decides when the owner must say yes first).
    async fn settle_changes(&self, to: &str, changes: &str, ctx: &ToolContext) -> ToolResult {
        let land = match changes {
            "land" => true,
            "discard" => false,
            other => {
                return ToolResult::error(format!(
                    "changes is \"land\" or \"discard\", not \"{other}\". Nothing was changed."
                ));
            }
        };
        if !matches!(self.recipient(to), Recipient::Helper) {
            return ToolResult::error("Only a helper you started holds changes to land or discard. Nothing was changed.");
        }
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        match orch.settle(to, land, ctx).await {
            Ok(text) => ToolResult::ok(text),
            Err(e) => ToolResult::error(e),
        }
    }

    /// A message into a coworker's own session — their persona, memory,
    /// connected accounts and permissions — through the coworker rail. It
    /// never waits: the message is queued, the call returns, and the reply
    /// comes back later as a notification. `conversation` picks which of a
    /// linked employee's conversations it goes into.
    async fn to_coworker(
        &self,
        ctx: &ToolContext,
        to: &str,
        text: &str,
        conversation: Option<crate::coworker::Conversation>,
    ) -> ToolResult {
        let rail = self.rail.read().unwrap().clone();
        let Some(rail) = rail else {
            return ToolResult::error(
                "Coworker messaging is not available in this environment (no coworker rail wired; \
                 use send_loop_message for bots on the NeboAI hub).",
            );
        };
        match crate::coworker::deliver(&rail, ctx, to, text, conversation).await {
            Ok(delivery) => {
                // Structured payload → the chat renders a first-class
                // "Messaged {name}" event (clickable through to the coworker
                // thread) instead of a bare tool chip.
                let payload = json!({
                    "kind": "coworker_message",
                    "to": delivery.to_name,
                    "toAgentId": delivery.to_agent_id,
                    "threadKey": delivery.thread_key,
                    "text": text,
                    "handoffId": delivery.handoff_id,
                });
                ToolResult::ok(format!(
                    "Message sent to {name}. They work on it in their own session and answer once; their answer \
                     comes to you as a notification, with any other answers you are waiting for, when the last \
                     one is in. Until then you know nothing about their answer: don't report, guess or redo it. \
                     If the owner asks, say {name} is working on it.",
                    name = delivery.to_name
                ))
                .with_payload(payload)
            }
            Err(e) => ToolResult::error(e),
        }
    }
}

/// The one employee a helper call hands its work TO, if exactly one: the
/// helper type is an employee's name, or the prompt addresses one — it opens
/// with the name, casts the helper as it ("you are …", "act as …"), or asks
/// for it to do the work ("ask …", "tell …", "have …"). A name the task only
/// mentions — a product to research, a colleague's past work — hands
/// nothing off (live 2026-09-28: "Search Hacker News for discussions about
/// … Claude Code …" was refused because a linked employee has that name).
/// Whole words only, and never inside a path: a prompt that points at
/// "/Library/Application Support/Nebo/sessions/..." is not asking for the
/// employee called Nebo (live Auto-Categorizer thread, 2026-09-06).
/// One helper per piece, each briefed with the shared `prompt` and its own
/// task, all in the background: the many-helpers launch, so a wide fan-out
/// is one call that states the context once rather than a response that
/// repeats it per helper. Each piece starts through the same spawn as a
/// single helper, with its own `report_limit` when it sets one; the result
/// lists every receipt.
async fn fan_out(orch: &dyn SubAgentOrchestrator, base: SpawnRequest, pieces: &[Value]) -> ToolResult {
    if base.wait {
        return ToolResult::error("Pieces always run in the background. Nothing was started. Send it again without `background: false`.");
    }
    let mut lines = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let task = piece["task"].as_str().unwrap_or("").trim();
        let description = piece["description"].as_str().unwrap_or("").trim();
        let req = SpawnRequest {
            prompt: format!("{}\n\nYour piece: {task}", base.prompt),
            description: description.to_string(),
            report_limit: match report_limit(piece) {
                0 => base.report_limit,
                own => own,
            },
            ..base.clone()
        };
        lines.push(match orch.spawn(req).await {
            Ok(r) => format!("{description}: {}", r.output),
            Err(e) => format!("{description}: couldn't start: {e}"),
        });
    }
    ToolResult::ok(lines.join("\n"))
}

/// The `report_limit` a delegate call or one of its pieces set, in
/// characters; 0 when it set none.
fn report_limit(input: &Value) -> usize {
    input["report_limit"].as_u64().unwrap_or(0) as usize
}

fn employee_addressed(prompt: &str, helper_type: &str, names: &[String]) -> Option<String> {
    let names: Vec<&String> = names.iter().filter(|n| n.trim().len() >= 3).collect();
    if let Some(n) = names.iter().find(|n| n.trim().eq_ignore_ascii_case(helper_type.trim())) {
        return Some((*n).clone());
    }
    let cleaned: String = prompt
        .split_whitespace()
        .filter(|w| !w.contains('/'))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let addressed = |n: &str| -> bool {
        let n = n.to_ascii_lowercase();
        cleaned.match_indices(&n).any(|(i, _)| {
            let before = cleaned[..i].chars().next_back();
            let after = cleaned[i + n.len()..].chars().next();
            // "nebo-cli" and "nebo_home" are compounds, not the name.
            let joins = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
            if before.is_some_and(joins) || after.is_some_and(joins) {
                return false;
            }
            addresses(&cleaned[..i])
        })
    };
    let mut hits = names.iter().filter(|n| addressed(n));
    match (hits.next(), hits.next()) {
        (Some(n), None) => Some((*n).clone()),
        _ => None,
    }
}

/// Whether the words before a name make the name the one the work is
/// handed to: nothing before it (or a greeting), a verb that asks someone
/// to act, or a phrase that casts the helper as them.
fn addresses(before: &str) -> bool {
    let mut words: Vec<&str> = before
        .split(|c: char| c.is_whitespace() || c == '@' || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    if matches!(words.last(), Some(&("the" | "our"))) {
        words.pop();
    }
    match words.as_slice() {
        [] | ["hey" | "hi" | "dear"] => true,
        [.., "ask" | "tell" | "have" | "get" | "message" | "ping" | "email" | "contact" | "let"] => true,
        [.., "you", "are"] | [.., "act" | "acting", "as"] | [.., "to", "be"] | [.., "role", "of"] => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperOp {
    Delegate,
    SendMessage,
}

impl HelperOp {
    const ALL: [HelperOp; 2] = [HelperOp::Delegate, HelperOp::SendMessage];
}

struct HelperTool {
    op: HelperOp,
    helpers: Arc<Helpers>,
}

impl DynTool for HelperTool {
    fn name(&self) -> &str {
        match self.op {
            HelperOp::Delegate => "delegate",
            HelperOp::SendMessage => "send_message",
        }
    }

    /// `delegate` says what the conversation keeps (the owner, the
    /// decisions, small direct steps) and what goes to a helper, runs in the
    /// background by default, never predicts a pending result, says what a
    /// brief holds (the deliverable and its length, and whether the helper
    /// may change things), and that a helper's question for the owner comes
    /// back to be asked. Its worked examples span the work employees do: a
    /// research batch in pieces with the summary drafted here, an ad with a
    /// helper per shot (the film skill's limits), an outreach batch, a code
    /// change, and "Still waiting" when asked mid-wait. What the schema
    /// already says (the default type, `background`, `isolation`) is said
    /// there only.
    fn description(&self) -> String {
        match self.op {
            HelperOp::Delegate => "Starts a helper on a piece of work in the background; you keep talking with the owner.\n\
                 When to use: the work matches a helper type, means reading across many sources, produces something substantial, splits into pieces that run side by side, or runs long. When the owner asks for a helper, start it first.\n\
                 When not to use: small direct things: one lookup, status or calendar check, short edit or reply, one known file or skill. Do those yourself. Once work is delegated, don't also do it yourself.\n\
                 - Only its final report comes back, as a notification. Until then you know nothing about the result. Never predict it; if the owner asks, say it's still running.\n\
                 - It sees only its brief: the goal, what you know or ruled out, what to leave alone, whether it may change things, and the exact deliverable with its length.\n\
                 - One job in many pieces: one call, the brief once in `prompt`, a task each in `pieces`.\n\
                 - It can't ask the owner: its question comes back in its report. Ask the owner, then send_message its id the decision in `answer`. Work for a named employee is a message to them.\n\
                 Examples: 30 listings → `pieces` of 10, one line per listing back; you draft the summary. A 30-second ad → a helper per shot, at most 4 at once, each reporting its clip and pass or fail; you compose. Outreach → a helper drafts the emails to a file and reports the path; the owner approves sending. A code change → the files, the change and the tests; it reports what changed and the test result.\n\
                 Owner, while one runs: \"Is Elm Street in?\" → \"Still waiting on the research; that's one it checks.\""
                .to_string(),
            HelperOp::SendMessage => "Sends a message to a helper you started (by its id), a coworker (another employee on this Nebo, by name) or a team (by name).\n\
                 - A running helper sees it at its next step; a finished one continues with it, keeping its context.\n\
                 - The owner's decision on a helper's question goes in `answer` (its id in `question`, else the newest); nothing else settles it.\n\
                 - A coworker gets it in their own session and answers with their own tools and permissions; their answer comes to you as a notification. Several messages in one response go out together.\n\
                 - To a team: only teammates it addresses receive it (`mention`, @Name, @everyone); naming nobody sends it to the lead.\n\
                 - Work for a named employee is a message to them, never a helper. Bots on the NeboAI hub are send_loop_message.\n\
                 - For a quick question to a coworker or two. A direction that spans employees, teams or days and must be carried to one outcome is a temporary workflow: create_workflow(lifetime: \"temporary\")."
                .to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            HelperOp::Delegate => json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "What the helper does, in 3-5 words the owner will read." },
                    "prompt": { "type": "string", "description": "The whole job: what to do, what you already know, and what to report back." },
                    "helper_type": { "type": "string", "enum": HELPER_TYPES, "description": "general (default); explore and plan only look and never change anything." },
                    "background": { "type": "boolean", "default": true, "description": "Run in the background and report by notification. false only when your very next step needs the result." },
                    "isolation": { "type": "string", "enum": ["worktree"], "description": "Its own copy of the folder it changes (code or documents): the owner's files change only when you land its changes." },
                    "speed": { "type": "string", "description": "The speed the helper works at, by model name. Leave it out and it works at yours." },
                    "report_limit": { "type": "integer", "description": format!("Its report is read in full up to this many characters (default and most: {REPORT_LIMIT}); past it, its start and a file.") },
                    "pieces": {
                        "type": "array",
                        "description": "A background helper per piece, briefed with `prompt` then its task. Any number.",
                        "items": {
                            "type": "object",
                            "properties": { "description": { "type": "string" }, "task": { "type": "string" }, "report_limit": { "type": "integer" } },
                            "required": ["description", "task"]
                        }
                    }
                },
                "required": ["description", "prompt"]
            }),
            HelperOp::SendMessage => json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "A helper's id (from delegate), a coworker's name, or a team's name." },
                    "message": { "type": "string", "description": "What to tell them." },
                    "answer": { "type": "string", "description": "To a helper: the owner's answer to its open question." },
                    "question": { "type": "string", "description": "With `answer`: the question's id." },
                    "mention": { "type": "array", "items": { "type": "string" }, "description": "To a team: who receives it, by name." },
                    "conversation": { "type": "string", "description": "A linked employee's conversation id (as list_employees shows it) or \"new\"; leave out to continue your thread with it." },
                    "changes": { "type": "string", "enum": ["land", "discard"], "description": "To a helper with its own copy: land its changes in the owner's files, or drop them." }
                },
                "required": ["to"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            HelperOp::Delegate => "start a helper on separate work",
            HelperOp::SendMessage => "message a helper coworker or team",
        }
    }

    /// Both are always loaded: handing off is always an option, and
    /// send_message is how work reaches a coworker; the proof runs of 2026-09-26 loaded it mid-conversation in the most runs, and each mid-conversation load rewrites the cached prompt (the core
    /// budget test in the registry has the counts).
    fn should_defer(&self) -> bool {
        false
    }

    /// Starting a helper changes nothing by itself (read-only and
    /// concurrency-safe; the helper's own calls are checked): parallel helpers are several
    /// delegate calls in one response.
    fn read_only(&self, _input: &Value) -> bool {
        self.op == HelperOp::Delegate
    }

    /// Every call returns at once, the message side too: a send only
    /// delivers, and the reply is a notification. So several sends in one
    /// response go out side by side, started as the reply streams.
    fn concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    /// Helpers are the employee's own work; a coworker or a team acts on
    /// the message with its own permissions.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    /// A message to a coworker or a team names who it goes to: what a
    /// recipient rule matches. A helper is the employee's own.
    fn rule_field(&self, input: &Value) -> Option<types::permissions::RuleField> {
        if self.op != HelperOp::SendMessage {
            return None;
        }
        match self.helpers.recipient(input["to"].as_str().unwrap_or("").trim()) {
            Recipient::Team(name) | Recipient::Coworker(name) => Some(types::permissions::RuleField::Recipient(name)),
            Recipient::Helper => None,
        }
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let blank = |k: &str| input[k].as_str().is_none_or(|s| s.trim().is_empty());
        let empty: &[&str] = match self.op {
            HelperOp::Delegate => &["description", "prompt"],
            // Settling a helper's changes needs no message.
            HelperOp::SendMessage if input["changes"].is_string() => &["to"],
            HelperOp::SendMessage => &["to", "message"],
        };
        if let Some(k) = empty.iter().find(|k| blank(k)) {
            return Err(format!("{k} can't be empty."));
        }
        match input["pieces"].as_array() {
            Some(pieces) if pieces.is_empty() => Err("pieces can't be empty: leave it out to start one helper.".to_string()),
            Some(pieces) => match pieces.iter().position(|p| ["description", "task"].iter().any(|k| p[*k].as_str().is_none_or(|s| s.trim().is_empty()))) {
                Some(i) => Err(format!("piece {} needs a description and a task.", i + 1)),
                None => Ok(()),
            },
            None => Ok(()),
        }
    }

    fn describe(&self, input: &Value) -> String {
        let desc = crate::humanize::cap(input["description"].as_str().unwrap_or(""), crate::humanize::DESCRIPTION_CAP);
        match self.op {
            HelperOp::Delegate => format!("starting a helper: {desc}"),
            HelperOp::SendMessage => format!("messaging {}", self.helpers.recipient_label(input)),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                HelperOp::Delegate => self.helpers.delegate(&input, ctx).await,
                HelperOp::SendMessage => self.helpers.send_message(&input, ctx).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::SpawnResult;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Records what each helper call asked the orchestrator for.
    #[derive(Default)]
    struct Recorder {
        spawned: Mutex<Vec<SpawnRequest>>,
        sent: Mutex<Vec<(String, String, String)>>,
    }

    /// What the harness says for a helper that is still working: at launch,
    /// or a foreground one that outlasted its budget.
    const RECEIPT: &str = "Helper h1 is working in the background. You'll get a notification when it finishes.";

    fn done(output: &str) -> SpawnResult {
        SpawnResult {
            task_id: "h1".into(),
            success: true,
            output: output.into(),
            error: None,
            taint: Vec::new(),
        }
    }

    impl SubAgentOrchestrator for Arc<Recorder> {
        fn spawn(&self, req: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
            // A prompt with "slow" in it outlasts the foreground budget.
            let background = !req.wait || req.prompt.contains("slow");
            let read_the_web = req.prompt.contains("web page");
            self.spawned.lock().unwrap().push(req);
            Box::pin(async move {
                Ok(if background {
                    done(RECEIPT)
                } else if read_the_web {
                    SpawnResult {
                        taint: vec![types::provenance::ProvenanceClass::Web],
                        ..done("helper h1 \"read the page\": done\nthe page says 40% off")
                    }
                } else {
                    done("helper h1 \"map the repo\": done\nthe report\n\nTo continue it, use send_message to h1.")
                })
            })
        }
        fn start_work(&self, req: SpawnRequest, _work: crate::orchestrator::Work) -> Fut<'_, Result<SpawnResult, String>> {
            self.spawned.lock().unwrap().push(req);
            Box::pin(async { Ok(done(RECEIPT)) })
        }
        fn cancel(&self, _task_id: &str, _caller: &str) -> Fut<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self, task_id: &str, _caller: &str) -> Fut<'_, Result<String, String>> {
            let known = task_id == "h1";
            Box::pin(async move {
                if known {
                    Ok("Task: h1\nStatus: running".into())
                } else {
                    Err("unknown".into())
                }
            })
        }
        fn send(
            &self,
            task_id: &str,
            message: &str,
            _answer: Option<crate::orchestrator::Answer>,
            parent: SpawnRequest,
        ) -> Fut<'_, Result<FollowUp, String>> {
            self.sent.lock().unwrap().push((
                task_id.into(),
                message.into(),
                parent.parent_session_key,
            ));
            let task_id = task_id.to_string();
            Box::pin(async move { Ok(FollowUp::Delivered { task_id }) })
        }
        fn list_active(&self, _caller: &str) -> Fut<'_, Vec<(String, String, String)>> {
            Box::pin(async { Vec::new() })
        }
        fn settle<'a>(&'a self, task_id: &'a str, land: bool, _ctx: &'a crate::ToolContext) -> Fut<'a, Result<String, String>> {
            let task_id = task_id.to_string();
            Box::pin(async move { Ok(format!("settled {task_id}: {}", if land { "landed" } else { "discarded" })) })
        }
        fn recover(&self) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    /// Records what reached the coworker rail.
    #[derive(Default)]
    struct Rail {
        sent: Mutex<Vec<(String, String)>>,
        conversations: Mutex<Vec<Option<crate::coworker::Conversation>>>,
        posts: Mutex<Vec<(String, String, Vec<String>)>>,
    }

    impl crate::coworker::CoworkerRail for Rail {
        fn send(&self, msg: crate::coworker::CoworkerMessage) -> Fut<'_, Result<crate::coworker::CoworkerDelivery, String>> {
            self.sent.lock().unwrap().push((msg.to.clone(), msg.text.clone()));
            self.conversations.lock().unwrap().push(msg.conversation.clone());
            Box::pin(async move {
                Ok(crate::coworker::CoworkerDelivery {
                    to_agent_id: "bk".into(),
                    to_name: msg.to,
                    thread_key: "agent:bk:coworker".into(),
                    handoff_id: Some("h-1".into()),
                })
            })
        }
        fn post_team(&self, post: crate::coworker::TeamPost) -> Fut<'_, Result<crate::coworker::TeamPostReceipt, String>> {
            self.posts.lock().unwrap().push((post.team_id.clone(), post.text.clone(), post.mention.clone()));
            Box::pin(async move {
                Ok(crate::coworker::TeamPostReceipt {
                    team_id: post.team_id,
                    team_name: "Back Office".into(),
                    message_id: "m1".into(),
                    asked: vec!["Bookkeeper".into()],
                })
            })
        }
        fn stop_team(&self, _stop: crate::coworker::TeamStop) -> Fut<'_, Result<String, String>> {
            Box::pin(async { Err("no team here".into()) })
        }
        fn company_now(&self, _query: crate::company::CompanyQuery) -> Fut<'_, Vec<crate::company::EmployeeNow>> {
            Box::pin(async { Vec::new() })
        }
    }

    struct Rig {
        tools: Vec<Box<dyn DynTool>>,
        rec: Arc<Recorder>,
        rail: Arc<Rail>,
        store: Arc<Store>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::new(&dir.path().join("h.db").to_string_lossy()).unwrap());
            let rec = Arc::new(Recorder::default());
            let handle = crate::orchestrator::new_handle();
            let _ = handle.set(Box::new(rec.clone()));
            let rail = Arc::new(Rail::default());
            let cell = crate::coworker::new_rail_cell();
            *cell.write().unwrap() = Some(rail.clone() as Arc<dyn crate::coworker::CoworkerRail>);
            let teams = Arc::new(Teams::new(Some(store.clone()), None, None, cell.clone()));
            let tools = Helpers::new(store.clone(), handle, teams, cell).tools();
            Self {
                tools,
                rec,
                rail,
                store,
                _dir: dir,
            }
        }

        async fn call(&self, name: &str, input: Value) -> ToolResult {
            let ctx = ToolContext {
                session_key: "agent:a1:web".into(),
                model_preference: Some("janus/fast".into()),
                ..Default::default()
            };
            let tool = self.tools.iter().find(|t| t.name() == name).unwrap();
            match tool.validate_input(&input) {
                Err(e) => ToolResult::error(e),
                Ok(()) => tool.execute_dyn(&ctx, input).await,
            }
        }
    }

    #[test]
    fn delegate_and_send_message_are_core() {
        let rig = Rig::new();
        let names: Vec<(&str, bool)> = rig
            .tools
            .iter()
            .map(|t| (t.name(), t.should_defer()))
            .collect();
        assert_eq!(
            names,
            [
                ("delegate", false),
                ("send_message", false)
            ]
        );
    }

    /// Background is the default: the helper reports by notification and
    /// the launch result says so; the child inherits the conversation's
    /// model.
    #[tokio::test]
    async fn a_helper_runs_in_the_background_by_default() {
        let rig = Rig::new();
        let r = rig
            .call(
                "delegate",
                json!({"description": "find the config", "prompt": "Find where the port is set."}),
            )
            .await;
        assert_eq!(r.content, RECEIPT, "the harness's receipt, as it is");
        let spawned = rig.rec.spawned.lock().unwrap();
        assert!(!spawned[0].wait);
        assert_eq!(spawned[0].agent_type, "general");
        assert_eq!(spawned[0].model_override, "janus/fast");
    }

    #[tokio::test]
    async fn a_foreground_helper_returns_its_report_and_how_to_continue() {
        let rig = Rig::new();
        let r = rig
            .call("delegate", json!({"description": "map the repo", "prompt": "List the crates.", "helper_type": "explore", "background": false}))
            .await;
        assert!(
            r.content.contains("the report") && r.content.contains("send_message to h1"),
            "{}",
            r.content
        );
        assert_eq!(r.content.matches("send_message").count(), 1, "one way to continue, said once: {}", r.content);
        let spawned = rig.rec.spawned.lock().unwrap();
        assert!(spawned[0].wait);
        assert_eq!(spawned[0].agent_type, "explore");
    }

    /// One call fans out: a helper per piece, each briefed with the shared
    /// prompt and its own task, all in the background.
    #[tokio::test]
    async fn pieces_start_a_helper_each_with_the_shared_brief() {
        let rig = Rig::new();
        let pieces: Vec<Value> = (1..=150).map(|i| json!({"description": format!("rival {i}"), "task": format!("Map rival {i}.")})).collect();
        let r = rig
            .call("delegate", json!({"description": "map rivals", "prompt": "Report pricing and positioning.", "pieces": pieces}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content.lines().count(), 150, "a receipt per piece");
        let spawned = rig.rec.spawned.lock().unwrap();
        assert_eq!(spawned.len(), 150);
        assert!(spawned.iter().all(|s| !s.wait && s.model_override == "janus/fast"));
        assert_eq!(spawned[41].prompt, "Report pricing and positioning.\n\nYour piece: Map rival 42.");
        assert_eq!(spawned[41].description, "rival 42");
        drop(spawned);

        let r = rig.call("delegate", json!({"description": "d", "prompt": "p", "pieces": [{"description": "a", "task": " "}]})).await;
        assert!(r.is_error && r.content.contains("piece 1"), "{}", r.content);
        let r = rig.call("delegate", json!({"description": "d", "prompt": "p", "background": false, "pieces": [{"description": "a", "task": "t"}]})).await;
        assert!(r.is_error && r.content.contains("background"), "{}", r.content);
        assert_eq!(rig.rec.spawned.lock().unwrap().len(), 150, "a refused call starts nothing");
    }

    /// Review 5.1: a finished helper's report carries what it read. The
    /// result the parent reads is marked with it, so the parent's run takes
    /// the helper's taint.
    #[tokio::test]
    async fn a_foreground_helpers_report_carries_what_it_read() {
        let rig = Rig::new();
        let r = rig
            .call("delegate", json!({"description": "read the page", "prompt": "Read the sale web page.", "background": false}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.taint, vec![types::provenance::ProvenanceClass::Web]);
    }

    /// An isolated helper gets its own copy, and runs in the background like
    /// any other (isolation doesn't decide foreground or background). Before:
    /// every worktree helper held its parent's step.
    #[tokio::test]
    async fn an_isolated_helper_works_in_its_own_copy_in_the_background() {
        let rig = Rig::new();
        let r = rig.call("delegate", json!({"description": "fix the tests", "prompt": "Fix them.", "isolation": "worktree"})).await;
        assert_eq!(r.content, RECEIPT);
        let spawned = rig.rec.spawned.lock().unwrap();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].isolate, "worktree");
        assert!(!spawned[0].wait, "isolation does not force the foreground");
    }

    /// A foreground helper that outlasts its budget moves to the background,
    /// and the tool says so: never "finished" or "merged back" for work
    /// still running.
    #[tokio::test]
    async fn a_foreground_helper_that_moved_to_the_background_is_not_reported_finished() {
        let rig = Rig::new();
        let r = rig
            .call("delegate", json!({"description": "port it", "prompt": "a slow port", "background": false, "isolation": "worktree"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, RECEIPT, "what actually happened: it is still working");
        assert!(rig.rec.spawned.lock().unwrap()[0].wait, "it was asked for in the foreground");
    }

    #[tokio::test]
    async fn a_prompt_naming_an_employee_is_refused() {
        let rig = Rig::new();
        rig.store
            .create_agent("cos", None, "Chief of Staff", "", "", "", None, None)
            .unwrap();
        let r = rig.call("delegate", json!({"description": "memo", "prompt": "Ask the Chief of Staff to draft the memo"})).await;
        assert!(
            r.is_error && r.content.contains("\"Chief of Staff\" is an employee"),
            "{}",
            r.content
        );
        assert!(rig.rec.spawned.lock().unwrap().is_empty());
    }

    /// The live refusal (2026-09-28): a research helper whose task mentions
    /// a product that shares a linked employee's name runs.
    #[tokio::test]
    async fn a_prompt_that_only_mentions_an_employee_runs() {
        let rig = Rig::new();
        rig.store.create_agent("cc", None, "Claude Code", "", "", "", None, None).unwrap();
        let r = rig
            .call(
                "delegate",
                json!({"description": "HN research", "prompt": "Search Hacker News for discussions about AI coding agents — Cursor, Claude Code, Codex — and summarize the themes."}),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(rig.rec.spawned.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_reaches_the_helper() {
        let rig = Rig::new();
        let r = rig
            .call(
                "send_message",
                json!({"to": "h1", "message": "also cover the edge cases"}),
            )
            .await;
        assert!(
            r.content.starts_with("Delivered to helper [h1]"),
            "{}",
            r.content
        );
        assert_eq!(
            rig.rec.sent.lock().unwrap()[0].2,
            "agent:a1:web",
            "the sender is recorded"
        );
    }

    #[tokio::test]
    async fn blank_required_text_is_refused_before_anything_runs() {
        let rig = Rig::new();
        assert!(
            rig.call("delegate", json!({"description": " ", "prompt": "x"}))
                .await
                .is_error
        );
        assert!(
            rig.call("send_message", json!({"to": "h1", "message": ""}))
                .await
                .is_error
        );
        assert!(
            rig.rec.spawned.lock().unwrap().is_empty() && rig.rec.sent.lock().unwrap().is_empty()
        );
    }

    #[test]
    fn employees_are_named_as_words_not_paths() {
        let names = vec!["Nebo".to_string(), "Chief of Staff".to_string()];
        let path_prompt = "Parse the file at /Users/a/Library/Application Support/Nebo/sessions/x/y.txt and summarize it.";
        assert_eq!(employee_addressed(path_prompt, "", &names), None);
        assert_eq!(
            employee_addressed("Ask the Chief of Staff to draft the memo", "", &names).as_deref(),
            Some("Chief of Staff")
        );
        assert_eq!(employee_addressed("Compare nebo-cli flags", "", &names), None);
    }

    /// Only work handed TO an employee is refused: the helper type is its
    /// name, the prompt opens with it, casts the helper as it, or asks it to
    /// act. A name the task only mentions runs as a helper — the live prompt
    /// that researched a product sharing a linked employee's name.
    #[test]
    fn only_work_handed_to_an_employee_is_refused() {
        let names = vec!["Claude Code".to_string(), "Chief of Staff".to_string()];
        let live = "Search Hacker News for discussions about AI coding agents from the last month — \
                    what people say about Cursor, Claude Code and Codex — and summarize the themes.";
        assert_eq!(employee_addressed(live, "general", &names), None);
        assert_eq!(employee_addressed("Summarize what Chief of Staff wrote last week", "", &names), None);
        for handed in [
            "Chief of Staff, draft the board memo",
            "@Chief of Staff draft the board memo",
            "You are the Chief of Staff. Draft the board memo.",
            "Act as Chief of Staff and draft the memo",
            "Have Claude Code fix the failing test",
            "Tell the Chief of Staff the memo is due",
        ] {
            assert!(employee_addressed(handed, "", &names).is_some(), "{handed}");
        }
        assert_eq!(employee_addressed("draft the memo", "chief of staff", &names).as_deref(), Some("Chief of Staff"));
    }

    /// One send tool, three kinds of `to`: a team's name posts into the
    /// team (with the members asked by name), an employee's name reaches
    /// the coworker in their own session, anything else is a helper's id.
    #[tokio::test]
    async fn send_message_routes_by_the_kind_of_to() {
        let rig = Rig::new();
        for (id, name) in [("bk", "Bookkeeper"), ("ea", "Executive Assistant")] {
            rig.store.create_agent(id, None, name, "d", "# agent", "", None, None).unwrap();
        }
        rig.store
            .create_team("t-1", "Back Office", "Books", &[db::TeamMember::local("bk"), db::TeamMember::local("ea")], "bk", None)
            .unwrap();

        let team = rig.call("send_message", json!({"to": "Back Office", "message": "close the month", "mention": ["Bookkeeper"]})).await;
        assert!(!team.is_error && team.content.contains("Posted to team \"Back Office\""), "{}", team.content);
        assert!(team.content.contains("Sent to Bookkeeper."), "the receipt names exactly who received it: {}", team.content);
        assert_eq!(rig.rail.posts.lock().unwrap()[0], ("t-1".to_string(), "close the month".to_string(), vec!["bk".to_string()]));

        let coworker = rig.call("send_message", json!({"to": "Bookkeeper", "message": "send the invoice"})).await;
        assert!(!coworker.is_error && coworker.content.starts_with("Message sent to Bookkeeper."), "{}", coworker.content);
        assert_eq!(rig.rail.sent.lock().unwrap()[0], ("Bookkeeper".to_string(), "send the invoice".to_string()));

        let helper = rig.call("send_message", json!({"to": "h1", "message": "also the edge cases"})).await;
        assert!(!helper.is_error, "{}", helper.content);
        assert_eq!(rig.rec.sent.lock().unwrap().len(), 1, "only the helper id reached the orchestrator");

        let send = rig.tools.iter().find(|t| t.name() == "send_message").unwrap();
        assert_eq!(send.rule_field(&json!({"to": "Bookkeeper"})), Some(types::permissions::RuleField::Recipient("Bookkeeper".into())));
        assert_eq!(send.rule_field(&json!({"to": "h1"})), None);
        assert_eq!(send.describe(&json!({"to": "Back Office"})), "messaging the Back Office team");
    }

    /// `conversation` reaches the rail as which of a linked employee's
    /// conversations the message goes into: one by id, or a new one; left
    /// out, the sender's own thread. A team or a helper has one
    /// conversation, so naming one there sends nothing.
    #[tokio::test]
    async fn a_message_names_which_of_its_conversations_it_goes_into() {
        use crate::coworker::Conversation;
        let rig = Rig::new();
        rig.store.create_agent("cx", Some("linked"), "Coder", "d", "# agent", "", None, None).unwrap();
        rig.store.create_team("t-1", "Back Office", "Books", &[db::TeamMember::local("cx")], "cx", None).unwrap();
        for (conversation, want) in [
            (json!("s-1"), Some(Conversation::Existing("s-1".into()))),
            (json!("NEW"), Some(Conversation::New)),
            (json!(""), None),
            (Value::Null, None),
        ] {
            let r = rig.call("send_message", json!({"to": "Coder", "message": "how far along?", "conversation": conversation})).await;
            assert!(!r.is_error, "{}", r.content);
            assert_eq!(rig.rail.conversations.lock().unwrap().pop().unwrap(), want);
        }
        for to in ["Back Office", "h1"] {
            let r = rig.call("send_message", json!({"to": to, "message": "x", "conversation": "s-1"})).await;
            assert!(r.is_error && r.content.contains("Nothing was sent"), "{to}: {}", r.content);
        }
        assert!(rig.rail.posts.lock().unwrap().is_empty() && rig.rec.sent.lock().unwrap().is_empty());
    }

    /// A send to a coworker never waits: there
    /// is no way to ask it to, it answers with a receipt, and several sends
    /// in one response run side by side.
    #[tokio::test]
    async fn a_coworker_message_never_waits() {
        let rig = Rig::new();
        rig.store.create_agent("bk", None, "Bookkeeper", "d", "# agent", "", None, None).unwrap();
        let send = rig.tools.iter().find(|t| t.name() == "send_message").unwrap();
        assert!(send.schema()["properties"].get("wait").is_none(), "one way: a send never waits");
        assert!(send.concurrency_safe(&json!({"to": "Bookkeeper", "message": "x"})));
        let r = rig.call("send_message", json!({"to": "Bookkeeper", "message": "the invoice"})).await;
        assert!(r.content.contains("answer once; their answer comes to you as a notification"), "{}", r.content);
        assert!(r.payload.as_ref().is_some_and(|p| p.get("reply").is_none()), "{:?}", r.payload);
    }

    /// D16: delegate says what the conversation keeps and what a helper
    /// takes, that a pending result is never predicted, what a brief holds
    /// (the deliverable and its length, and whether it may change things),
    /// and that a helper's question comes back to be asked of the owner.
    #[test]
    fn delegate_says_when_to_hand_off() {
        let rig = Rig::new();
        let delegate = rig.tools.iter().find(|t| t.name() == "delegate").unwrap().description();
        for part in [
            "When to use: the work matches a helper type, means reading across many sources, produces something substantial, splits into pieces that run side by side, or runs long.",
            "When not to use: small direct things: one lookup, status or calendar check, short edit or reply, one known file or skill. Do those yourself.",
            "Once work is delegated, don't also do it yourself.",
            "Never predict it; if the owner asks, say it's still running.",
            "whether it may change things, and the exact deliverable with its length",
            "It can't ask the owner: its question comes back in its report. Ask the owner, then send_message its id the decision in `answer`.",
            "Still waiting on the research",
        ] {
            assert!(delegate.contains(part), "{part:?} missing from:\n{delegate}");
        }
        assert!(delegate.chars().count() <= 1_600, "a lean description: {}", delegate.chars().count());
    }

    /// The worked examples span the work employees do, not only code: a
    /// research batch in pieces with the summary drafted in the
    /// conversation, an ad with a helper per shot (the film skill's limit
    /// of 4 at once), an outreach batch the owner approves, and a code
    /// change. A wide fan-out of one job is one call with `pieces`.
    #[test]
    fn delegate_examples_span_the_work() {
        let rig = Rig::new();
        let delegate = rig.tools.iter().find(|t| t.name() == "delegate").unwrap().description();
        for part in [
            "One job in many pieces: one call, the brief once in `prompt`, a task each in `pieces`.",
            "30 listings → `pieces` of 10, one line per listing back; you draft the summary.",
            "A 30-second ad → a helper per shot, at most 4 at once",
            "Outreach → a helper drafts the emails to a file and reports the path; the owner approves sending.",
            "A code change → the files, the change and the tests; it reports what changed and the test result.",
        ] {
            assert!(delegate.contains(part), "{part:?} missing from:\n{delegate}");
        }
    }

    /// `question` names the question an `answer` settles: alone, it is
    /// refused and nothing is sent.
    #[tokio::test]
    async fn a_question_without_an_answer_is_refused() {
        let rig = Rig::new();
        let r = rig.call("send_message", json!({"to": "h1", "message": "x", "question": "q-1"})).await;
        assert!(r.is_error && r.content.contains("no `answer` was given"), "{}", r.content);
        assert!(rig.rec.sent.lock().unwrap().is_empty(), "nothing sent");
    }

    /// A delegate call sets how long its helper's report may be, for the
    /// whole call or per piece; the schema says the default, which is also
    /// the most.
    #[tokio::test]
    async fn report_limit_is_set_per_call_and_per_piece() {
        let rig = Rig::new();
        let schema = rig.tools.iter().find(|t| t.name() == "delegate").unwrap().schema();
        assert_eq!(schema["properties"]["report_limit"]["type"], "integer");
        assert!(schema["properties"]["report_limit"]["description"].as_str().unwrap().contains(&format!("default and most: {REPORT_LIMIT}")));
        assert_eq!(schema["properties"]["pieces"]["items"]["properties"]["report_limit"]["type"], "integer");

        let r = rig.call("delegate", json!({"description": "one listing", "prompt": "Check it.", "report_limit": 3_000})).await;
        assert!(!r.is_error, "{}", r.content);
        let pieces = json!([
            {"description": "batch 1", "task": "Listings 1-15."},
            {"description": "batch 2", "task": "Listings 16-30.", "report_limit": 1_500},
        ]);
        let r = rig.call("delegate", json!({"description": "listings", "prompt": "One line each.", "report_limit": 4_000, "pieces": pieces})).await;
        assert!(!r.is_error, "{}", r.content);
        let r = rig.call("delegate", json!({"description": "no limit", "prompt": "Check it."})).await;
        assert!(!r.is_error, "{}", r.content);
        let limits: Vec<usize> = rig.rec.spawned.lock().unwrap().iter().map(|s| s.report_limit).collect();
        assert_eq!(limits, [3_000, 4_000, 1_500, 0], "the call's, a piece's own, and none set");
    }
}
