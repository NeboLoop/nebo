//! After the turn: memory extraction scheduling, personality synthesis, the
//! chat title and the background tool summary, moved here from `runner.rs`
//! (WP1.5). WP2.7 turns extraction into a background pass over the finished
//! exchange with no pre-gate and adds the review fork.

use std::collections::BTreeSet;
use std::sync::Arc;

use ai::{Provider, RequestTrace, StreamEvent};
use db::Store;
use napp::agent::MemoryTopic;
use tokio::sync::{RwLock, mpsc};
use tools::ToolResult;
use tracing::{debug, info};
use types::provenance::ProvenanceClass;

use crate::concurrency::ConcurrencyController;
use crate::harness::model_call::{prefer_non_gateway, resolve_aux};
use crate::memory;
use crate::session::SessionManager;

/// Sink for a freshly auto-generated chat title. The runner writes the title to
/// the store itself; the server installs a sink (`set_title_sink`) that
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
/// the user set. Entered from the run loop after each turn and from
/// Runner::spawn_title_generation for chats whose turns are persisted outside
/// a run (voice).
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
        if messages.len() < 2 {
            return; // need a user+assistant exchange to name from
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

/// What a finished run hands the memory extraction pass.
pub(crate) struct MemoryExtraction<'a> {
    pub sessions: &'a SessionManager,
    pub session_id: &'a str,
    pub providers: &'a Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    pub store: &'a Arc<Store>,
    pub concurrency: &'a Arc<ConcurrencyController>,
    pub embedding_provider: Option<&'a Arc<dyn ai::EmbeddingProvider>>,
    /// The gate's judge: the runner's own decide handle (the one client the
    /// server builds).
    pub decide: Option<&'a Arc<ai::DecideClient>>,
    pub memory_user_id: &'a str,
    pub memory_topics: &'a [MemoryTopic],
    pub memory_write_bar: &'a [ProvenanceClass],
    pub run_taint: &'a std::sync::Mutex<BTreeSet<ProvenanceClass>>,
    /// The objective line: evidence for the gate.
    pub objective: &'a str,
    pub skip_memory: bool,
    pub gate_trace: RequestTrace,
    pub trace: RequestTrace,
}

impl MemoryExtraction<'_> {
    /// Debounced memory extraction: only runs after 5s idle per session.
    /// Extract from last exchange only (last user msg + assistant response + tool
    /// calls) to avoid re-extracting facts from old messages and creating duplicates.
    pub(crate) async fn schedule(self) {
        let session_id = self.session_id;
        let has_providers = !self.providers.read().await.is_empty();
        let final_taint: Vec<ProvenanceClass> =
            self.run_taint.lock().unwrap().iter().copied().collect();
        let extraction_barred = final_taint
            .iter()
            .any(|c| self.memory_write_bar.contains(c));
        if extraction_barred {
            info!(
                session_id,
                classes = %types::provenance::label_classes(&final_taint),
                "memory extraction barred by scope write bar"
            );
        }
        if !self.skip_memory && has_providers && !extraction_barred {
            let all_msgs = self.sessions.get_messages(session_id).unwrap_or_default();
            // Find the last user message and take everything from there onward.
            let last_exchange: Vec<_> = {
                let last_user_idx = all_msgs.iter().rposition(|m| m.role == "user");
                match last_user_idx {
                    Some(idx) => all_msgs[idx..].to_vec(),
                    None => vec![],
                }
            };
            if last_exchange.len() >= 2 {
                use crate::memory_debounce::MemoryDebouncer;
                use std::sync::OnceLock;
                static DEBOUNCER: OnceLock<MemoryDebouncer> = OnceLock::new();
                let debouncer = DEBOUNCER.get_or_init(MemoryDebouncer::default);

                let providers = self.providers.clone();
                let store = self.store.clone();
                let mem_uid = self.memory_user_id.to_string();
                let session_id_owned = session_id.to_string();
                let embed_prov = self.embedding_provider.cloned();
                let topics = self.memory_topics.to_vec();
                let taint = final_taint.clone();
                let conc = self.concurrency.clone();
                let decide = self.decide.cloned();
                let objective = self.objective.to_string();
                let gate_trace = self.gate_trace;
                let trace = self.trace;

                debouncer
                    .schedule(session_id, move || async move {
                        // One typed decision before the chat-model extraction:
                        // skip only when the new turn plausibly holds nothing
                        // durable; every doubt runs extraction as before.
                        let gate_state = crate::memory_gate::gate_state(&last_exchange, &objective);
                        if !crate::memory_gate::should_extract(
                            decide.as_deref(),
                            &gate_trace,
                            &gate_state,
                        )
                        .await
                        {
                            debug!(
                                session_id = session_id_owned,
                                "memory extraction skipped: nothing durable in the turn"
                            );
                            return;
                        }
                        let resolved = {
                            let prov_lock = providers.read().await;
                            resolve_aux(&config::ModelsConfig::load(), &prov_lock)
                                .or_else(|| {
                                    prefer_non_gateway(&prov_lock).map(|p| (p, String::new()))
                                })
                                .map(|(p, m)| (conc.background(p), m))
                        };
                        if let Some((provider, aux_model)) = resolved
                            && let Some(facts) = memory::extract_facts(
                                trace,
                                provider.as_ref(),
                                &last_exchange,
                                Some(&store),
                                Some(&mem_uid),
                                &topics,
                                &aux_model,
                            )
                            .await
                        {
                            memory::store_facts(
                                &store, &facts, &mem_uid, embed_prov, &topics, &taint,
                            );
                            debug!(
                                session_id = session_id_owned,
                                "extracted and stored memory facts"
                            );
                        }
                    })
                    .await;
            }
        }
    }
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
