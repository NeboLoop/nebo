//! Helpers: `delegate` starts one, `send_message` talks to a running or
//! finished one — or, by the kind of `to`, to a coworker or a team — and
//! `orchestrate` runs a decomposed job as a dependency graph of helpers. Behaviour stays in the orchestrator; these are its
//! interface. Reading a helper's output and stopping it are `read_output`
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
        let isolated = input["isolation"].as_str() == Some("worktree");
        let req = SpawnRequest {
            prompt: input["prompt"].as_str().unwrap_or("").to_string(),
            description: input["description"].as_str().unwrap_or("").to_string(),
            agent_type: input["helper_type"]
                .as_str()
                .unwrap_or("general")
                .to_string(),
            wait: !background || isolated,
            isolate: if isolated {
                "worktree".to_string()
            } else {
                String::new()
            },
            ..SpawnRequest::child_of(ctx)
        };
        if isolated {
            // An isolated helper runs in its own copy of the project, merged
            // back when it finishes: the batch path does the copy and merge.
            let progress = match ctx.stream_tx.clone() {
                Some(tx) => tx,
                None => tokio::sync::mpsc::channel(16).0,
            };
            return match orch.spawn_parallel(vec![req], progress).await {
                Ok(r) if r.success => ToolResult::ok(format!(
                    "Helper [{}] finished in its own copy of the project; its changes are merged back.\n\n{}",
                    r.task_id, r.output
                )),
                Ok(r) => ToolResult::error(format!(
                    "Helper [{}] failed:\n\n{}\n\n{}",
                    r.task_id,
                    r.output,
                    r.error.unwrap_or_default()
                )),
                Err(e) => ToolResult::error(format!("Couldn't start the helper: {e}")),
            };
        }
        match orch.spawn(req).await {
            Ok(r) if r.success && background => {
                ToolResult::ok(format!("Helper [{}]: {}", r.task_id, r.output))
            }
            Ok(r) if r.success => ToolResult::ok(format!(
                "Helper [{id}] finished:\n\n{out}\n\nTo refine, extend or correct this, \
                 send_message to {id}: it keeps its context and the files it read. Start a new \
                 helper only for unrelated work.",
                id = r.task_id,
                out = r.output
            )),
            Ok(r) => ToolResult::error(format!(
                "Helper [{}] failed: {}",
                r.task_id,
                r.error.unwrap_or_default()
            )),
            Err(e) => ToolResult::error(format!("Couldn't start the helper: {e}")),
        }
    }

    async fn orchestrate(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        // The nodes of a decomposition are this run's own helpers: they sit,
        // run at the model, and are limited like a single delegate.
        match orch
            .execute_dag(
                input["prompt"].as_str().unwrap_or(""),
                SpawnRequest::child_of(ctx),
            )
            .await
        {
            Ok(r) if r.success => ToolResult::ok(format!(
                "Orchestration [{}] completed:\n\n{}",
                r.task_id, r.output
            )),
            Ok(r) => ToolResult::error(format!(
                "Orchestration [{}] had failures:\n\n{}\n\nError: {}",
                r.task_id,
                r.output,
                r.error.unwrap_or_default()
            )),
            Err(e) => ToolResult::error(format!("Orchestration failed: {e}")),
        }
    }

    async fn send_message(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let to = input["to"].as_str().unwrap_or("").trim();
        let message = input["message"].as_str().unwrap_or("").trim();
        match self.recipient(to) {
            Recipient::Team(team) => return self.teams.post(ctx, &team, message, &input["mention"]).await,
            Recipient::Coworker(name) => return self.to_coworker(ctx, &name, message, input["wait"].as_bool().unwrap_or(true)).await,
            Recipient::Helper => {}
        }
        let orch = match self.orchestrator() {
            Ok(o) => o,
            Err(e) => return ToolResult::error(e),
        };
        match orch
            .send(
                to,
                message,
                &ctx.session_key,
                ctx.run_taint.clone(),
                Some(ctx.cancel_token.clone()),
                ctx.stream_tx.clone(),
            )
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
    /// connected accounts and permissions — through the coworker rail.
    async fn to_coworker(&self, ctx: &ToolContext, to: &str, text: &str, wait: bool) -> ToolResult {
        let rail = self.rail.read().unwrap().clone();
        let Some(rail) = rail else {
            return ToolResult::error(
                "Coworker messaging is not available in this environment (no coworker rail wired; \
                 use send_loop_message for bots on the NeboAI hub).",
            );
        };
        match crate::coworker::deliver(&rail, ctx, to, text, wait).await {
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
                    "reply": delivery.reply.clone(),
                });
                match delivery.reply {
                    Some(ref reply) => ToolResult::ok(format!(
                        "Message delivered to {}. Their reply:\n\n{}",
                        delivery.to_name, reply
                    ))
                    .with_payload(payload),
                    None => ToolResult::ok(format!(
                        "Message delivered to {} — they are handling it in their own session; \
                         their reply reaches you when it comes. Until then, report this as \
                         \"asked {} — waiting\", never as done.",
                        delivery.to_name, delivery.to_name
                    ))
                    .with_payload(payload),
                }
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
    Orchestrate,
    SendMessage,
}

impl HelperOp {
    const ALL: [HelperOp; 3] = [
        HelperOp::Delegate,
        HelperOp::Orchestrate,
        HelperOp::SendMessage,
    ];
}

struct HelperTool {
    op: HelperOp,
    helpers: Arc<Helpers>,
}

impl DynTool for HelperTool {
    fn name(&self) -> &str {
        match self.op {
            HelperOp::Delegate => "delegate",
            HelperOp::Orchestrate => "orchestrate",
            HelperOp::SendMessage => "send_message",
        }
    }

    fn description(&self) -> String {
        match self.op {
            HelperOp::Delegate => "Starts a helper on a self-contained piece of work. Helper types are listed in reminders: general (the default), explore and plan (these two only look).\n\
                 - Helpers run in the background by default; you're notified when one finishes and only its final report comes back. Don't guess its results before then.\n\
                 - Set `background: false` only when your very next step needs the result.\n\
                 - Brief it fully: it hasn't seen this conversation. Say what to do, what you already know, and what to report back.\n\
                 - Several independent pieces: several delegate calls in one response. Helpers that edit files in the same project: `isolation: \"worktree\"` gives each its own copy, merged back when it finishes.\n\
                 - To continue a running or finished helper, use send_message with its id.\n\
                 - If you already know the file or answer, use the direct tool instead. Work for a named employee is a message to them, not a helper."
                .to_string(),
            HelperOp::Orchestrate => "Breaks a large job into steps that depend on each other and runs each step as a helper, in order, handing each step what the earlier ones found. Returns the combined result.\n\
                 - For independent pieces, several delegate calls are faster."
                .to_string(),
            HelperOp::SendMessage => "Sends a message to a helper you started (by its id), a coworker (another employee on this Nebo, by name) or a team (by name).\n\
                 - A running helper sees it at its next step; a finished one continues with it, keeping its context.\n\
                 - A coworker gets it in their own session and answers with their own tools and permissions. `wait: false` doesn't wait for the reply; it reaches you when it comes.\n\
                 - A team's lead answers and hands steps to teammates; `mention` asks named members to act, and @everyone in the message asks the whole team.\n\
                 - Work for a named employee is a message to them, never a helper. Bots on the NeboAI hub are send_loop_message."
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
                    "isolation": { "type": "string", "enum": ["worktree"], "description": "Give the helper its own copy of the project, merged back when it finishes. Use it when helpers edit files in the same project." }
                },
                "required": ["description", "prompt"]
            }),
            HelperOp::Orchestrate => json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "The whole job, with everything the steps need to know." }
                },
                "required": ["prompt"]
            }),
            HelperOp::SendMessage => json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "A helper's id (from delegate), a coworker's name, or a team's name." },
                    "message": { "type": "string", "description": "What to tell them." },
                    "wait": { "type": "boolean", "default": true, "description": "To a coworker: wait for their reply (default). false sends it and carries on." },
                    "mention": { "type": "array", "items": { "type": "string" }, "description": "To a team: the members asked to act, by name." }
                },
                "required": ["to", "message"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            HelperOp::Delegate => "start a helper on separate work",
            HelperOp::Orchestrate => "run dependent steps as helpers",
            HelperOp::SendMessage => "message a helper coworker or team",
        }
    }

    /// `delegate` is always loaded (Claude Code's Agent); the rest are found
    /// with find_tools.
    fn should_defer(&self) -> bool {
        self.op != HelperOp::Delegate
    }

    /// Starting a helper changes nothing by itself (Claude Code marks Agent
    /// read-only and concurrency-safe): parallel helpers are several
    /// delegate calls in one response.
    fn read_only(&self, _input: &Value) -> bool {
        self.op == HelperOp::Delegate
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
            HelperOp::Orchestrate => &["prompt"],
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
            HelperOp::Orchestrate => "running a multi-step job".to_string(),
            HelperOp::SendMessage => format!("messaging {}", self.helpers.recipient_label(input)),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        let desc = input["description"].as_str().unwrap_or("");
        match self.op {
            HelperOp::Delegate => format!("Started a helper: {desc}"),
            HelperOp::Orchestrate => "Ran a multi-step job".to_string(),
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
                HelperOp::Orchestrate => self.helpers.orchestrate(&input, ctx).await,
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
    use tokio::sync::mpsc;

    type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Records what each helper call asked the orchestrator for.
    #[derive(Default)]
    struct Recorder {
        spawned: Mutex<Vec<SpawnRequest>>,
        batches: Mutex<Vec<Vec<SpawnRequest>>>,
        dags: Mutex<Vec<(String, SpawnRequest)>>,
        sent: Mutex<Vec<(String, String, String)>>,
    }

    fn done(output: &str) -> SpawnResult {
        SpawnResult {
            task_id: "h1".into(),
            success: true,
            output: output.into(),
            error: None,
        }
    }

    impl SubAgentOrchestrator for Arc<Recorder> {
        fn spawn(&self, req: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
            let background = !req.wait;
            self.spawned.lock().unwrap().push(req);
            Box::pin(async move {
                Ok(done(if background {
                    "working in the background"
                } else {
                    "the report"
                }))
            })
        }
        fn execute_dag(
            &self,
            prompt: &str,
            parent: SpawnRequest,
        ) -> Fut<'_, Result<SpawnResult, String>> {
            self.dags.lock().unwrap().push((prompt.to_string(), parent));
            Box::pin(async { Ok(done("all steps")) })
        }
        fn cancel(&self, _task_id: &str) -> Fut<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self, task_id: &str) -> Fut<'_, Result<String, String>> {
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
            from_session_key: &str,
            _taint: Vec<types::provenance::ProvenanceClass>,
            _parent_cancel: Option<tokio_util::sync::CancellationToken>,
            _parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
        ) -> Fut<'_, Result<FollowUp, String>> {
            self.sent.lock().unwrap().push((
                task_id.into(),
                message.into(),
                from_session_key.into(),
            ));
            let task_id = task_id.to_string();
            Box::pin(async move { Ok(FollowUp::Delivered { task_id }) })
        }
        fn list_active(&self) -> Fut<'_, Vec<(String, String, String)>> {
            Box::pin(async { Vec::new() })
        }
        fn spawn_parallel(
            &self,
            requests: Vec<SpawnRequest>,
            _progress_tx: mpsc::Sender<ai::StreamEvent>,
        ) -> Fut<'_, Result<SpawnResult, String>> {
            self.batches.lock().unwrap().push(requests);
            Box::pin(async { Ok(done("merged")) })
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
                    reply: Some("on it".into()),
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
                ("orchestrate", true),
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
        assert_eq!(r.content, "Helper [h1]: working in the background");
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
        let spawned = rig.rec.spawned.lock().unwrap();
        assert!(spawned[0].wait);
        assert_eq!(spawned[0].agent_type, "explore");
    }

    /// An isolated helper gets its own copy through the batch path.
    #[tokio::test]
    async fn an_isolated_helper_works_in_its_own_copy() {
        let rig = Rig::new();
        let r = rig.call("delegate", json!({"description": "fix the tests", "prompt": "Fix them.", "isolation": "worktree"})).await;
        assert!(
            !r.is_error && r.content.contains("merged back"),
            "{}",
            r.content
        );
        let batches = rig.rec.batches.lock().unwrap();
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].isolate, "worktree");
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
    async fn send_and_orchestrate_reach_the_orchestrator() {
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
        let r = rig
            .call("orchestrate", json!({"prompt": "research then write"}))
            .await;
        assert!(r.content.contains("all steps"), "{}", r.content);
        assert_eq!(
            rig.rec.dags.lock().unwrap()[0].1.model_override,
            "janus/fast",
            "nodes inherit the model"
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
        assert!(!coworker.is_error && coworker.content.contains("Their reply:\n\non it"), "{}", coworker.content);
        assert_eq!(rig.rail.sent.lock().unwrap()[0], ("Bookkeeper".to_string(), "send the invoice".to_string()));

        let helper = rig.call("send_message", json!({"to": "h1", "message": "also the edge cases"})).await;
        assert!(!helper.is_error, "{}", helper.content);
        assert_eq!(rig.rec.sent.lock().unwrap().len(), 1, "only the helper id reached the orchestrator");

        let send = rig.tools.iter().find(|t| t.name() == "send_message").unwrap();
        assert_eq!(send.rule_field(&json!({"to": "Bookkeeper"})), Some(types::permissions::RuleField::Recipient("Bookkeeper".into())));
        assert_eq!(send.rule_field(&json!({"to": "h1"})), None);
        assert_eq!(send.activity(&json!({"to": "Back Office"})), "messaging the Back Office team");
    }
}
