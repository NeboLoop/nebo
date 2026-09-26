//! After the turn: memory extraction, personality synthesis, the chat title,
//! the self-improvement review and the background tool summary.
//!
//! Memory extraction: after each turn, one background call over the
//! messages since the session's last extraction writes the durable
//! memories it finds. There is no pre-gate. It is skipped when the employee already saved memory itself
//! during those messages. One pass runs per session at a time; a turn that
//! ends meanwhile becomes the trailing pass, which starts where the running
//! one stopped.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use ai::{Provider, RequestTrace, StreamEvent};
use db::Store;
use db::models::ChatMessage;
use napp::agent::MemoryTopic;
use tokio::sync::{RwLock, mpsc};
use tools::ToolResult;
use tracing::{debug, info};
use types::provenance::ProvenanceClass;

use crate::concurrency::ConcurrencyController;
use crate::harness::model_call::{prefer_non_gateway, resolve_aux};
use crate::memory;
use crate::session::SessionManager;

/// Sink for a freshly auto-generated chat title. The harness writes the title
/// to the store itself; the server installs a sink (`Harness::bind`) that
/// broadcasts the change to connected clients and propagates it to the loop —
/// concerns the agent crate can't reach. ONE sink, set once at startup, used by
/// every run path (replaces the per-path title generators + the skip_title_gen
/// flag). Implementations must not block (spawn for async work).
pub trait ChatTitleSink: Send + Sync {
    fn on_title(&self, session_key: String, chat_id: String, title: String);
}

/// The ONE chat-title generator body (CODE_AUDITOR Rule 8). Names the chat on
/// its first user turn and refines once at the third — language-independent
/// (message count, not a default-title string) — and never clobbers a title
/// the user set. Entered after each chat turn and from
/// `Harness::spawn_title_generation` for chats whose turns are stored outside
/// a turn (voice).
pub(crate) fn spawn_chat_title_generation(
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    store: Arc<Store>,
    chat_id: String,
    session_id: String,
    cheap_model: String,
    title_sink: Option<Arc<dyn ChatTitleSink>>,
) {
    tokio::spawn(async move {
        let chat = match store.get_chat(&chat_id) {
            Ok(Some(c)) => c,
            _ => return,
        };
        // Never clobber a title the user explicitly set.
        if chat.title_custom {
            return;
        }
        // Gate on user turns across the WHOLE chat, not the recent window: a
        // windowed count kept re-hitting 1 or 3 as the conversation grew,
        // re-titling the chat from whatever the user said most recently.
        let user_turns = match store.count_chat_user_messages(&chat_id) {
            Ok(n) => n as usize,
            _ => return,
        };
        if user_turns != 1 && user_turns != 3 {
            return; // name once, refine once — at most twice
        }
        let messages = match store.get_recent_chat_messages(&chat_id, 8) {
            Ok(m) => m,
            _ => return,
        };
        if !messages.iter().any(|m| m.role == "user") {
            return; // the owner's words name it, answered or not
        }
        // Use more of the conversation on the count-3 refinement.
        let take_n = if user_turns >= 3 { 8 } else { 4 };
        let transcript: String = messages
            .iter()
            .take(take_n)
            .map(|m| {
                let snippet: String = m.content.chars().take(200).collect();
                format!("{}: {}", m.role, snippet)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(title) = crate::summarizer::generate_session_title(
            RequestTrace::new("title"),
            &providers,
            &transcript,
            &cheap_model,
        )
        .await
        {
            let _ = store.update_chat_title(&chat_id, &title, false);
            info!(chat_id = %chat_id, title = %title, "auto-generated chat title");
            if let Some(sink) = title_sink {
                sink.on_title(session_id, chat_id, title);
            }
        }
    });
}

/// Minimum gap between background tool-summary labels for one session.
///
/// The label is a one-line UX caption ("Read auth config and fixed token
/// validation"). It was spawned once per tool-executing iteration, which made
/// it **30.1% of all LLM requests** in the 2026-08-27 incident (3,597 of
/// 11,946) — a third of the traffic for a caption. At a normal working pace one
/// label per round still lands; in a fast loop the captions were arriving
/// faster than a human could read them anyway.
const TOOL_SUMMARY_MIN_GAP: std::time::Duration = std::time::Duration::from_secs(15);

/// Last tool-summary label spawned per session.
static TOOL_SUMMARY_LAST: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::LazyLock::new(Default::default);

/// Whether to spawn a tool-summary label for this iteration. Rate-limited per
/// session; the label is cosmetic, so skipping one costs nothing but the
/// caption for that round.
fn tool_summary_due(session_id: &str, now: std::time::Instant) -> bool {
    let mut last = TOOL_SUMMARY_LAST.lock().unwrap_or_else(|p| p.into_inner());
    match last.get(session_id) {
        Some(prev) if now.duration_since(*prev) < TOOL_SUMMARY_MIN_GAP => false,
        _ => {
            last.insert(session_id.to_string(), now);
            true
        }
    }
}

/// Background tool summary generation via cheap model (Pattern 13).
/// Spawns a fire-and-forget task that calls the cheapest provider to
/// generate a one-line label for the UX showing what the agent did.
/// Rate-limited per session (see TOOL_SUMMARY_MIN_GAP) — ungated this
/// was a third of all LLM requests.
pub(crate) async fn hand_off_tool_summary(
    session_id: &str,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tx: &mpsc::Sender<StreamEvent>,
    assistant_content: &str,
    tool_calls: Vec<ai::ToolCall>,
    tool_results: Vec<ToolResult>,
    trace: RequestTrace,
) {
    if tool_summary_due(session_id, std::time::Instant::now()) {
        let prov_lock = providers.read().await;
        let prov_snapshot: Vec<Arc<dyn Provider>> = prov_lock.clone();
        drop(prov_lock);
        let summary_tx = tx.clone();
        let summary_assistant = assistant_content.to_string();
        tokio::spawn(async move {
            if let Some(summary) = crate::summarizer::summarize_tool_batch(
                trace,
                &prov_snapshot,
                &tool_calls,
                &tool_results,
                &summary_assistant,
            )
            .await
            {
                let _ = summary_tx.send(StreamEvent::tool_summary(summary)).await;
            }
        });
    }
}

/// What a finished turn hands the memory extraction service.
pub(crate) struct MemoryExtraction<'a> {
    pub sessions: &'a SessionManager,
    pub session_id: &'a str,
    pub providers: &'a Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    pub store: &'a Arc<Store>,
    pub concurrency: &'a Arc<ConcurrencyController>,
    /// The extraction model's window: the conversation fills half of it.
    pub selector: &'a Arc<crate::selector::ModelSelector>,
    pub embedding_provider: Option<&'a Arc<dyn ai::EmbeddingProvider>>,
    /// Tells a memory write in the conversation: a call whose rule key is
    /// `remember`.
    pub tools: &'a Arc<tools::Registry>,
    pub memory_user_id: &'a str,
    pub memory_topics: &'a [MemoryTopic],
    pub memory_write_bar: &'a [ProvenanceClass],
    pub run_taint: &'a std::sync::Mutex<BTreeSet<ProvenanceClass>>,
    /// The session's agreed goal, when it has one: the one piece of context
    /// extraction reads besides the messages.
    pub goal: Option<&'a str>,
    pub skip_memory: bool,
    pub trace: RequestTrace,
}

/// One extraction pass, owned so it can run in the background and wait as
/// a session's trailing pass.
struct ExtractionJob {
    sessions: SessionManager,
    session_id: String,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    store: Arc<Store>,
    concurrency: Arc<ConcurrencyController>,
    selector: Arc<crate::selector::ModelSelector>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    tools: Arc<tools::Registry>,
    memory_user_id: String,
    memory_topics: Vec<MemoryTopic>,
    taint: Vec<ProvenanceClass>,
    goal: Option<String>,
    trace: RequestTrace,
}

/// A session's extraction state: the cursor (the last message a pass
/// covered), whether a pass is running, and the pass that arrived while it
/// ran (only the latest is kept: it covers everything since the cursor).
#[derive(Default)]
struct SessionExtraction {
    cursor: Option<String>,
    running: bool,
    trailing: Option<ExtractionJob>,
}

static EXTRACTIONS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, SessionExtraction>>> =
    std::sync::LazyLock::new(Default::default);

fn extractions() -> std::sync::MutexGuard<'static, HashMap<String, SessionExtraction>> {
    EXTRACTIONS.lock().unwrap_or_else(|p| p.into_inner())
}

impl MemoryExtraction<'_> {
    /// Hand the finished turn to the extraction service. It runs in the
    /// background over the messages since the session's last extraction —
    /// no pre-gate. A write bar the run's taint crosses, a run that skips
    /// memory, or no provider hands nothing. A pass already running for the
    /// session keeps this one as its trailing pass.
    pub(crate) async fn schedule(self) {
        let session_id = self.session_id;
        let taint: Vec<ProvenanceClass> =
            self.run_taint.lock().unwrap().iter().copied().collect();
        if taint.iter().any(|c| self.memory_write_bar.contains(c)) {
            info!(
                session_id,
                classes = %types::provenance::label_classes(&taint),
                "memory extraction barred by scope write bar"
            );
            return;
        }
        if self.skip_memory || self.providers.read().await.is_empty() {
            return;
        }
        let job = ExtractionJob {
            sessions: self.sessions.clone(),
            session_id: session_id.to_string(),
            providers: self.providers.clone(),
            store: self.store.clone(),
            concurrency: self.concurrency.clone(),
            selector: self.selector.clone(),
            embedding_provider: self.embedding_provider.cloned(),
            tools: self.tools.clone(),
            memory_user_id: self.memory_user_id.to_string(),
            memory_topics: self.memory_topics.to_vec(),
            taint,
            goal: self.goal.map(str::to_string),
            trace: self.trace,
        };
        {
            let mut all = extractions();
            let state = all.entry(job.session_id.clone()).or_default();
            if state.running {
                debug!(session_id, "memory extraction running; this turn waits as its trailing pass");
                state.trailing = Some(job);
                return;
            }
            state.running = true;
        }
        let handle = tokio::spawn(run_extractions(job));
        crate::memory_flush::track_extraction(handle).await;
    }
}

/// Run `job`, then each trailing pass that arrived meanwhile.
async fn run_extractions(mut job: ExtractionJob) {
    loop {
        let cursor = extractions().get(&job.session_id).and_then(|s| s.cursor.clone());
        let advanced = extract_once(&job, cursor.as_deref()).await;
        let mut all = extractions();
        let state = all.entry(job.session_id.clone()).or_default();
        if advanced.is_some() {
            state.cursor = advanced;
        }
        match state.trailing.take() {
            Some(next) => job = next,
            None => {
                state.running = false;
                return;
            }
        }
    }
}

/// One pass over the messages after `cursor`. Returns the new cursor: the
/// last message the pass covered, or None when the pass failed and those
/// messages wait for the next one.
async fn extract_once(job: &ExtractionJob, cursor: Option<&str>) -> Option<String> {
    let all = job.sessions.get_messages(&job.session_id).unwrap_or_default();
    let last = all.last()?.id.clone();
    let messages = messages_since(&all, cursor);
    if messages.len() < 2 {
        return Some(last);
    }
    // The employee saved memory itself during these messages: it already
    // did the work this pass would do.
    if wrote_memory(&job.tools, &messages).await {
        debug!(session_id = %job.session_id, "memory extraction skipped: the employee wrote memory itself");
        return Some(last);
    }
    let resolved = {
        let prov_lock = job.providers.read().await;
        resolve_aux(&config::ModelsConfig::load(), &prov_lock)
            .or_else(|| prefer_non_gateway(&prov_lock).map(|p| (p, String::new())))
            .map(|(p, m)| {
                let window = job.selector.context_window(&format!("{}/{}", p.id(), m));
                (job.concurrency.background(p), m, window)
            })
    };
    let (provider, aux_model, window_tokens) = resolved?;
    let facts = memory::extract_facts(
        job.trace.clone(),
        provider.as_ref(),
        &messages,
        Some((job.store.as_ref(), job.memory_user_id.as_str())),
        &job.memory_topics,
        &aux_model,
        job.goal.as_deref(),
        window_tokens,
    )
    .await?;
    memory::store_facts(
        &job.store,
        &facts,
        &job.memory_user_id,
        job.embedding_provider.clone(),
        &job.memory_topics,
        &job.taint,
    );
    debug!(session_id = %job.session_id, "extracted and stored memory facts");
    Some(last)
}

/// The messages after `cursor`, attachment rows left out (they are the
/// system's words, not the conversation's). A cursor the conversation no
/// longer holds (a checkpoint, a new chat, a restart) falls back to the
/// last exchange: the last owner message and everything after it.
fn messages_since(all: &[ChatMessage], cursor: Option<&str>) -> Vec<ChatMessage> {
    let visible = |m: &&ChatMessage| crate::harness::reminders::attachment_kind(m).is_none();
    let start = match cursor.and_then(|c| all.iter().position(|m| m.id == c)) {
        Some(i) => i + 1,
        None => match all.iter().rposition(|m| m.role == "user" && visible(&m)) {
            Some(i) => i,
            None => return Vec::new(),
        },
    };
    all[start..].iter().filter(visible).cloned().collect()
}

/// Whether any assistant message calls a tool whose rule key is `remember`.
async fn wrote_memory(tools: &tools::Registry, messages: &[ChatMessage]) -> bool {
    for m in messages.iter().filter(|m| m.role == "assistant") {
        let Some(calls) = m
            .tool_calls
            .as_deref()
            .and_then(|tc| serde_json::from_str::<Vec<ai::ToolCall>>(tc).ok())
        else {
            continue;
        };
        for call in calls {
            if tools.target(&call.name, &call.input).await.is_some_and(|t| t.key == "remember") {
                return true;
            }
        }
    }
    false
}

/// Background personality synthesis: if enough style observations exist,
/// synthesize a personality directive. Runs at most once per run (spawned
/// as a background task so it doesn't block the response).
pub(crate) async fn spawn_personality_synthesis(
    store: &Arc<Store>,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    memory_user_id: &str,
    concurrency: &Arc<ConcurrencyController>,
) {
    let store_clone = store.clone();
    let providers_clone = providers.clone();
    let uid = memory_user_id.to_string();
    let conc = concurrency.clone();
    let handle = tokio::spawn(async move {
        let prov = prefer_non_gateway(&providers_clone.read().await).map(|p| conc.background(p));
        if let Some(prov) = prov {
            crate::personality::synthesize_directive(&store_clone, prov.as_ref(), &uid).await;
        }
    });
    crate::memory_flush::track_extraction(handle).await;
}

/// The self-improvement review after a chat turn. Every
/// `REVIEW_TURN_INTERVAL` turns without a skill the employee saved on its own,
/// an employee that learns (`learning_mode` auto or staged) gets a forked
/// turn over the conversation asking what should be learned. The fork runs
/// in its own session, never touches the owner's thread, and can only save
/// skills. One review runs per session at a time.
pub(crate) fn start_review(h: &super::Harness, req: &super::TurnRequest, session_id: &str) {
    let agent_id = req.seat.agent_id.as_str();
    if agent_id.is_empty() {
        return;
    }
    let learning = h
        .store
        .get_entity_config("agent", agent_id)
        .ok()
        .flatten()
        .and_then(|c| c.learning_mode)
        .map(|m| m.to_ascii_lowercase())
        .unwrap_or_default();
    let staged = learning == "staged";
    if !(staged || learning == "auto")
        || !crate::review_fork::should_review(session_id)
        || !crate::review_fork::try_begin(session_id)
    {
        return;
    }
    let h = h.clone();
    let fork = super::TurnRequest {
        session_key: format!("fork:{session_id}:review-{}", uuid::Uuid::new_v4()),
        input: super::TurnInput::Platform {
            text: crate::review_fork::REVIEW_PROMPT.to_string(),
        },
        seat: super::SeatRequest {
            origin: tools::Origin::System,
            ..req.seat.clone()
        },
        mode: super::TurnMode::Fork(super::ForkKind::Review { staged }),
        delivery: super::Delivery {
            channel: req.delivery.channel.clone(),
            channel_ctx: req.delivery.channel_ctx.clone(),
            mention_briefing: None,
        },
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: None,
    };
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        info!(session_id = %session_id, "self-improvement review starting");
        match review(&h, &session_id, fork).await {
            Ok(summary) => {
                let line: String = summary.trim().chars().take(300).collect();
                info!(session_id = %session_id, summary = %line, "self-improvement review finished");
            }
            Err(e) => tracing::warn!(session_id = %session_id, error = %e, "self-improvement review failed"),
        }
        crate::review_fork::finish(&session_id);
    });
}

/// Replay the conversation into the fork's session (so its request shares
/// the parent's cached prefix) and run the review turn to its end.
async fn review(h: &super::Harness, session_id: &str, fork: super::TurnRequest) -> Result<String, String> {
    let fork_session = h
        .sessions
        .get_or_create(&fork.session_key, &fork.seat.user_id)
        .map_err(|e| format!("the review session could not be created: {e}"))?;
    let messages = h
        .sessions
        .get_messages_since_checkpoint(session_id)
        .map_err(|e| format!("the conversation could not be read: {e}"))?;
    for m in &messages {
        h.sessions
            .append_message(
                &fork_session.id,
                &m.role,
                &m.content,
                m.tool_calls.as_deref(),
                m.tool_results.as_deref(),
                m.metadata.as_deref(),
            )
            .map_err(|e| format!("the conversation could not be replayed: {e}"))?;
    }
    let mut handle = h.start_turn(fork).await.map_err(|e| e.to_string())?;
    let mut text = String::new();
    while let Some(ev) = handle.events.recv().await {
        if ev.event_type == ai::StreamEventType::Text {
            text.push_str(&ev.text);
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tool-summary label is a caption, not work. Once per iteration made it
    /// 30.1% of all LLM requests in the incident; it is rate-limited per session.
    #[test]
    fn tool_summary_label_is_rate_limited() {
        let sid = "label-test-session";
        TOOL_SUMMARY_LAST.lock().unwrap().remove(sid);
        let t0 = std::time::Instant::now();

        assert!(tool_summary_due(sid, t0), "first label always lands");
        // A fast loop: ten tool rounds inside the gap produce no further labels.
        for i in 1..=10 {
            let t = t0 + std::time::Duration::from_secs(i);
            assert!(!tool_summary_due(sid, t), "no label {i}s into the gap");
        }
        // Past the gap, labelling resumes.
        assert!(
            tool_summary_due(sid, t0 + TOOL_SUMMARY_MIN_GAP),
            "label resumes after the gap"
        );
    }

    /// A provider that records each extraction prompt and answers with
    /// nothing to keep.
    #[derive(Default)]
    struct Recorder {
        prompts: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Provider for Recorder {
        fn id(&self) -> &str {
            "recorder"
        }
        async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            self.prompts
                .lock()
                .unwrap()
                .push(req.messages.iter().map(|m| m.content.clone()).collect());
            let (tx, rx) = mpsc::channel(4);
            let _ = tx.send(StreamEvent::text("{}".to_string())).await;
            let _ = tx.send(StreamEvent::done()).await;
            Ok(rx)
        }
    }

    struct Fixture {
        store: Arc<Store>,
        sessions: SessionManager,
        session_id: String,
        recorder: Arc<Recorder>,
        providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
        concurrency: Arc<ConcurrencyController>,
        selector: Arc<crate::selector::ModelSelector>,
        tools: Arc<tools::Registry>,
    }

    impl Fixture {
        async fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nebo-extract-{}.db", uuid::Uuid::new_v4()));
            let store = Arc::new(Store::new(path.to_str().unwrap()).expect("test store"));
            let sessions = SessionManager::new(store.clone());
            let session_id = sessions.get_or_create("agent:a1:web", "").expect("session").id;
            let recorder = Arc::new(Recorder::default());
            let providers: Arc<RwLock<Vec<Arc<dyn Provider>>>> =
                Arc::new(RwLock::new(vec![recorder.clone() as Arc<dyn Provider>]));
            let tools = Arc::new(tools::Registry::new(Arc::new(
                crate::harness::permissions::Check::new(store.clone()),
            )));
            for tool in tools::memory_tools::Memory::new(store.clone(), None, None).tools() {
                tools.register(tool).await;
            }
            Fixture {
                store,
                sessions,
                session_id,
                recorder,
                providers,
                concurrency: Arc::new(ConcurrencyController::new(None)),
                selector: Arc::new(crate::selector::ModelSelector::new(Default::default())),
                tools,
            }
        }

        fn say(&self, role: &str, text: &str, tool_calls: Option<serde_json::Value>) {
            let calls = tool_calls.map(|c| c.to_string());
            self.sessions
                .append_message(&self.session_id, role, text, calls.as_deref(), None, None)
                .expect("append");
        }

        /// End a turn: hand it to the service and wait for its passes.
        async fn end_turn(&self, goal: Option<&str>) {
            let taint = std::sync::Mutex::new(BTreeSet::new());
            MemoryExtraction {
                sessions: &self.sessions,
                session_id: &self.session_id,
                providers: &self.providers,
                store: &self.store,
                concurrency: &self.concurrency,
                selector: &self.selector,
                embedding_provider: None,
                tools: &self.tools,
                memory_user_id: "local:agent:a1",
                memory_topics: &[],
                memory_write_bar: &[],
                run_taint: &taint,
                goal,
                skip_memory: false,
                trace: RequestTrace::new("memory_extract"),
            }
            .schedule()
            .await;
            for _ in 0..400 {
                if !extractions().get(&self.session_id).is_some_and(|s| s.running) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("the extraction never finished");
        }

        fn prompts(&self) -> Vec<String> {
            self.recorder.prompts.lock().unwrap().clone()
        }
    }

    /// A status exchange with nothing durable in it — what the old Jev gate
    /// skipped — still gets its extraction call: nothing decides before it.
    #[tokio::test]
    async fn extraction_runs_without_a_gate() {
        let f = Fixture::new().await;
        f.say("user", "status?", None);
        f.say("assistant", "All quiet.", None);
        f.end_turn(None).await;
        assert_eq!(f.prompts().len(), 1, "one extraction call per turn, ungated");

        // The next turn covers only the messages since the last extraction.
        f.say("user", "My accountant is Dana Lee; send her the quarterly numbers.", None);
        f.say("assistant", "Noted.", None);
        f.end_turn(None).await;
        let prompts = f.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].contains("Dana Lee"));
        assert!(!prompts[1].contains("All quiet."), "the first turn is not extracted again");
    }

    /// When the employee saved memory itself during the messages, the pass
    /// has nothing to add: no call, and the cursor moves past them.
    #[tokio::test]
    async fn extraction_skipped_when_employee_wrote_memory() {
        let f = Fixture::new().await;
        f.say("user", "Remember that invoices go out on the 1st.", None);
        let store_call = serde_json::json!([{
            "id": "call-1",
            "name": "remember",
            "input": {"key": "invoice/day", "value": "Invoices go out on the 1st"}
        }]);
        f.say("assistant", "", Some(store_call));
        f.say("assistant", "Saved.", None);
        f.end_turn(None).await;
        assert!(f.prompts().is_empty(), "the employee already wrote memory");

        // A recall is not a write.
        f.say("user", "What day do invoices go out? I prefer email reminders.", None);
        let recall_call = serde_json::json!([{
            "id": "call-2",
            "name": "recall",
            "input": {"query": "invoice/day"}
        }]);
        f.say("assistant", "", Some(recall_call));
        f.say("assistant", "On the 1st.", None);
        f.end_turn(None).await;
        let prompts = f.prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("email reminders"));
        assert!(!prompts[0].contains("Remember that invoices"), "the written range is behind the cursor");
    }

    /// Extraction reads the messages and the agreed goal; attachment rows
    /// never reach it.
    #[tokio::test]
    async fn extraction_input_is_the_conversation_and_the_goal() {
        let f = Fixture::new().await;
        f.say("user", "Draft the renewal letter for the client.", None);
        let attachment = crate::harness::reminders::Attachment {
            kind: "relevant_memories",
            text: "Memories that may apply:\n- RECALLED-ROW: v".into(),
            data: serde_json::Map::new(),
        };
        f.sessions
            .append_message(
                &f.session_id,
                "user",
                &crate::harness::reminders::wrap(&attachment.text),
                None,
                None,
                Some(&attachment.metadata().to_string()),
            )
            .unwrap();
        f.say("assistant", "Here is the draft.", None);
        f.end_turn(Some("Send every renewal letter before Friday")).await;

        let prompts = f.prompts();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("The agreed goal of this work: Send every renewal letter before Friday"));
        assert!(prompts[0].contains("Draft the renewal letter"));
        assert!(!prompts[0].contains("RECALLED-ROW"), "attachment rows are not the conversation");

        // No goal, no goal line.
        f.say("user", "Also copy the partner.", None);
        f.say("assistant", "Done.", None);
        f.end_turn(None).await;
        assert!(!f.prompts()[1].contains("agreed goal"));
    }

    /// Sessions are rate-limited independently.
    #[test]
    fn tool_summary_limit_is_per_session() {
        let t = std::time::Instant::now();
        for sid in ["label-a", "label-b"] {
            TOOL_SUMMARY_LAST.lock().unwrap().remove(sid);
        }
        assert!(tool_summary_due("label-a", t));
        assert!(tool_summary_due("label-b", t));
    }
}
