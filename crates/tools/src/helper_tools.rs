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
use crate::orchestrator::{FollowUp, OrchestratorHandle, SpawnRequest, SubAgentOrchestrator};
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

    /// A helper has no name: a call that names an employee is work for a
    /// coworker, and an anonymous helper would impersonate them (smoke,
    /// 2026-09-05: "Chief of Staff" got a blank helper).
    fn names_an_employee(&self, input: &Value) -> Option<String> {
        let names: Vec<String> = self
            .store
            .list_agents(500, 0)
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.name)
            .collect();
        let who = employee_named_in_prompt(input["prompt"].as_str().unwrap_or(""), &names)?;
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
            ..SpawnRequest::child_of(ctx)
        };
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
        match self.recipient(to) {
            Recipient::Team(team) => return self.teams.post(ctx, &team, message, &input["mention"]).await,
            Recipient::Coworker(name) => return self.to_coworker(ctx, &name, message).await,
            Recipient::Helper => {}
        }
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        match orch
            .send(to, message, SpawnRequest::child_of(ctx))
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
    /// A message into a coworker's own session — their persona, memory,
    /// connected accounts and permissions — through the coworker rail. It
    /// never waits: the message is queued, the call returns, and the reply
    /// comes back later as a notification.
    async fn to_coworker(&self, ctx: &ToolContext, to: &str, text: &str) -> ToolResult {
        let rail = self.rail.read().unwrap().clone();
        let Some(rail) = rail else {
            return ToolResult::error(
                "Coworker messaging is not available in this environment (no coworker rail wired; \
                 use send_loop_message for bots on the NeboAI hub).",
            );
        };
        match crate::coworker::deliver(&rail, ctx, to, text).await {
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
                });
                ToolResult::ok(format!(
                    "Message sent to {name}. They work on it in their own session, and their reply \
                     comes to you as a notification. Until then you know nothing about their answer: \
                     don't report, guess or redo it. If the owner asks, say {name} is working on it.",
                    name = delivery.to_name
                ))
                .with_payload(payload)
            }
            Err(e) => ToolResult::error(e),
        }
    }
}

/// The one employee a prompt names, if exactly one. Whole words only, and
/// never inside a path: a prompt that points at
/// "/Library/Application Support/Nebo/sessions/..." is not asking for the
/// employee called Nebo (live Auto-Categorizer thread, 2026-09-06).
fn employee_named_in_prompt(prompt: &str, names: &[String]) -> Option<String> {
    let cleaned: String = prompt
        .split_whitespace()
        .filter(|w| !w.contains('/'))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let as_word = |n: &str| -> bool {
        let n = n.to_ascii_lowercase();
        cleaned.match_indices(&n).any(|(i, _)| {
            let before = cleaned[..i].chars().next_back();
            let after = cleaned[i + n.len()..].chars().next();
            // "nebo-cli" and "nebo_home" are compounds, not the name.
            let joins = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
            !before.is_some_and(joins) && !after.is_some_and(joins)
        })
    };
    let mut hits = names.iter().filter(|n| n.trim().len() >= 3 && as_word(n));
    match (hits.next(), hits.next()) {
        (Some(n), None) => Some(n.clone()),
        _ => None,
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

    /// `delegate` says when to use it and when not to, runs in the background
    /// by default, never predicts a pending result, says how to write the
    /// brief, and gives worked examples (a survey launched in the background
    /// with the report in a later turn, and "Still waiting on the audit" when
    /// asked mid-wait). Independent helpers are started together in one
    /// response, with a third example from the 2026-09-26 billing employee
    /// that loaded 28 skills one per step: a survey of many skills is helpers' reading,
    /// started together, and the parent builds from their digests. What the
    /// schema already says (the default type, `background`, `isolation`) is
    /// said there only.
    fn description(&self) -> String {
        match self.op {
            HelperOp::Delegate => "Starts a helper on a piece of work, so the conversation stays open while it runs. Helper types, and when each fits, are listed in reminders.\n\
                 When to use: the work matches a helper type, independent pieces can run side by side, or answering means reading across many files, pages or skills. You keep the conclusion, not the raw output. When the owner asks for a helper, start it first.\n\
                 When not to use: the target is known (a path, a name, a value, one skill): use read_file, run_command or use_skill. Once a search is delegated, don't also run it yourself.\n\
                 - It runs in the background: only its final report comes back, as a notification.\n\
                 - Until then you know nothing about the result. Never predict it; if the owner asks, say it's still running.\n\
                 - It hasn't seen this conversation. Brief it: the goal, what you know or ruled out, what to report. For a lookup, the exact command; for an investigation, the question.\n\
                 - Several pieces: several delegate calls in one response.\n\
                 - To continue a helper, send_message with its id. Work for a named employee is a message to them.\n\
                 Example: owner: \"Find every place the retry setting is used.\" → delegate(helper_type: \"explore\", ...), reply \"A helper is searching; I'll report when it's back.\", and the turn ends. The report comes in a later turn.\n\
                 Example: owner, before it's back: \"Is billing one?\" → \"Still waiting on the search; that's one of the things it checks.\"\n\
                 Example: twenty skills to learn before building → two delegate(helper_type: \"explore\") calls in one response, each reading half and sending back a digest."
                .to_string(),
            HelperOp::SendMessage => "Sends a message to a helper you started (by its id), a coworker (another employee on this Nebo, by name) or a team (by name).\n\
                 - A running helper sees it at its next step; a finished one continues with it, keeping its context.\n\
                 - A coworker gets it in their own session and answers with their own tools and permissions; their reply comes to you as a notification. Several messages in one response go out together.\n\
                 - A team's lead answers and hands steps to teammates; `mention` asks named members to act, and @everyone in the message asks the whole team.\n\
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
                    "isolation": { "type": "string", "enum": ["worktree"], "description": "Give the helper its own copy of the project, merged back when it finishes. Use it when helpers edit files in the same project." },
                    "speed": { "type": "string", "description": "The speed the helper works at, by model name. Leave it out and it works at yours." }
                },
                "required": ["description", "prompt"]
            }),
            HelperOp::SendMessage => json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "A helper's id (from delegate), a coworker's name, or a team's name." },
                    "message": { "type": "string", "description": "What to tell them." },
                    "mention": { "type": "array", "items": { "type": "string" }, "description": "To a team: the members asked to act, by name." }
                },
                "required": ["to", "message"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            HelperOp::Delegate => "start a helper on separate work",
            HelperOp::SendMessage => "message a helper coworker or team",
        }
    }

    /// `delegate` is always loaded (handing off is always an option); the rest are found
    /// with find_tools.
    fn should_defer(&self) -> bool {
        self.op != HelperOp::Delegate
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
            HelperOp::SendMessage => &["to", "message"],
        };
        match empty.iter().find(|k| blank(k)) {
            Some(k) => Err(format!("{k} can't be empty.")),
            None => Ok(()),
        }
    }

    fn activity(&self, input: &Value) -> String {
        let desc = input["description"].as_str().unwrap_or("");
        match self.op {
            HelperOp::Delegate => format!("starting a helper: {desc}"),
            HelperOp::SendMessage => format!("messaging {}", self.helpers.recipient_label(input)),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        let desc = input["description"].as_str().unwrap_or("");
        match self.op {
            HelperOp::Delegate => format!("Started a helper: {desc}"),
            HelperOp::SendMessage => format!("Messaged {}", self.helpers.recipient_label(input)),
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
        fn recover(&self) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    /// Records what reached the coworker rail.
    #[derive(Default)]
    struct Rail {
        sent: Mutex<Vec<(String, String)>>,
        posts: Mutex<Vec<(String, String, Vec<String>)>>,
    }

    impl crate::coworker::CoworkerRail for Rail {
        fn send(&self, msg: crate::coworker::CoworkerMessage) -> Fut<'_, Result<crate::coworker::CoworkerDelivery, String>> {
            self.sent.lock().unwrap().push((msg.to.clone(), msg.text.clone()));
            Box::pin(async move {
                Ok(crate::coworker::CoworkerDelivery {
                    to_agent_id: "bk".into(),
                    to_name: msg.to,
                    thread_key: "agent:bk:coworker".into(),
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
    fn delegate_is_core_and_the_rest_are_deferred() {
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
                ("send_message", true)
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

    /// Parity 5.1: a finished helper's report carries what it read. The
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
        assert_eq!(employee_named_in_prompt(path_prompt, &names), None);
        assert_eq!(
            employee_named_in_prompt("Ask the Chief of Staff to draft the memo", &names).as_deref(),
            Some("Chief of Staff")
        );
        assert_eq!(
            employee_named_in_prompt("Compare nebo-cli flags", &names),
            None
        );
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
        assert_eq!(send.activity(&json!({"to": "Back Office"})), "messaging the Back Office team");
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
        assert!(r.content.contains("their reply comes to you as a notification"), "{}", r.content);
        assert!(r.payload.as_ref().is_some_and(|p| p.get("reply").is_none()), "{:?}", r.payload);
    }

    /// D16: delegate says when to hand off and when not to, that a pending
    /// result is never predicted, and shows both worked examples; the old
    /// text only warned against using it.
    #[test]
    fn delegate_says_when_to_hand_off() {
        let rig = Rig::new();
        let delegate = rig.tools.iter().find(|t| t.name() == "delegate").unwrap().description();
        for part in [
            "When to use: the work matches a helper type, independent pieces can run side by side, or answering means reading across many files, pages or skills.",
            "When not to use: the target is known (a path, a name, a value, one skill): use read_file, run_command or use_skill.",
            "Once a search is delegated, don't also run it yourself.",
            "Never predict it; if the owner asks, say it's still running.",
            "delegate(helper_type: \"explore\", ...)",
            "Still waiting on the search",
        ] {
            assert!(delegate.contains(part), "{part:?} missing from:\n{delegate}");
        }
        assert!(!delegate.contains("If you already know the file or answer, use the direct tool instead."));
        assert!(delegate.chars().count() <= 1_600, "a lean description: {}", delegate.chars().count());
    }

    /// 2026-09-26: asked for a billing employee with all its workflows, an
    /// employee loaded 28 skills one step at a time and never delegated. The
    /// description now shows that survey as helpers started together in one
    /// response, each sending back a digest.
    #[test]
    fn delegate_shows_a_skill_survey_fanned_out() {
        let rig = Rig::new();
        let delegate = rig.tools.iter().find(|t| t.name() == "delegate").unwrap().description();
        for part in [
            "Several pieces: several delegate calls in one response.",
            "twenty skills to learn before building → two delegate(helper_type: \"explore\") calls in one response, each reading half and sending back a digest.",
        ] {
            assert!(delegate.contains(part), "{part:?} missing from:\n{delegate}");
        }
    }
}
