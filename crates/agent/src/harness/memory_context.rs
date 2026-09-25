//! Memory in the turn: the employee-memory section of the per-session
//! prompt (WP2.2) and the relevant-memories prefetch (WP2.7).
//!
//! The prefetch works the way Claude Code's relevant-memory prefetch works:
//! the search starts once per turn at Prepare and runs while the turn's
//! steps go on. Each step asks whether it has finished, without waiting;
//! the first step that finds it finished writes a `relevant_memories`
//! attachment, and every later step asks nothing. The attachment is a
//! persisted conversation row, so the recalled memories stay visible on
//! later steps and later turns. The row carries the ids it showed, and
//! [`surfaced_memories`] folds them back from the conversation, so a memory
//! surfaced once this session is never surfaced again (until a checkpoint
//! drops the row).

use std::collections::HashSet;
use std::sync::Arc;

use db::models::ChatMessage;
use tokio::task::JoinHandle;

use super::events::{MAX_RECALLED, TurnEvent};
use super::reminders::{self, Reminders, attachment_fields};
use crate::db_context::{self, InheritScope};
use crate::memory::ScoredMemory;

/// The attachment the prefetch writes.
const RELEVANT_MEMORIES: &str = "relevant_memories";

/// What the employee knows at the start of a turn: the identity slice of
/// its memory (profile, owner, preferences, learned personality, the
/// always-on memories) plus the owner's configured inputs.
pub struct EmployeeMemory {
    /// The `employee_memory` row's text.
    pub section: String,
    /// The owner's IANA timezone, when set: the environment's date is
    /// computed in it.
    pub timezone: Option<String>,
    /// The memories the section shows. Prepare counts them as accessed
    /// ([`record_access`]) and keeps them out of the recall ([`RecallRequest::skip`]).
    pub identity_ids: Vec<i64>,
}

/// Load the employee-memory section once per turn. `user_id` and
/// `inherit_scopes` come from the seat's memory scope, so an isolated seat
/// never sees a sibling's memories.
pub fn load_employee_memory(
    store: &db::Store,
    user_id: &str,
    agent_id: &str,
    inherit_scopes: &[InheritScope],
    agent_name: &str,
) -> EmployeeMemory {
    let ctx = db_context::load_db_context(store, user_id, agent_id, inherit_scopes);
    let timezone = ctx.user.as_ref().and_then(|u| u.timezone.clone()).filter(|tz| !tz.is_empty());
    let identity_ids = ctx.tacit_memories.iter().map(|m| m.memory.id).collect();
    let mut section = db_context::format_for_system_prompt(&ctx, agent_name);
    let inputs = if agent_id.is_empty() {
        None
    } else {
        store
            .get_agent(agent_id)
            .ok()
            .flatten()
            .and_then(|a| db_context::format_configured_inputs(&a.input_values))
    };
    if let Some(inputs) = inputs {
        section.push_str("\n\n---\n\n");
        section.push_str(&inputs);
    }
    EmployeeMemory {
        section,
        timezone,
        identity_ids,
    }
}

/// What the recall searches for and what it may show.
pub struct RecallRequest<'a> {
    /// The turn's owner message.
    pub prompt: &'a str,
    /// The seat's memory scope (`Seat::memory.user_id`): the search reads its
    /// scope chain only, so an isolated seat never reaches a sibling context.
    pub user_id: &'a str,
    /// Recall-for-audience (`Seat::audience_restricted`): only `tacit/`
    /// memories may surface.
    pub tacit_only: bool,
    /// Memories never to show: the identity slice and what this session has
    /// already surfaced.
    pub skip: HashSet<i64>,
}

/// The turn's relevant-memories prefetch, started at Prepare.
pub struct RecallPrefetch {
    search: Option<JoinHandle<Vec<ScoredMemory>>>,
}

impl RecallPrefetch {
    /// Start the search. Nothing to search for (an empty message, no
    /// searcher) gives a prefetch that never lands.
    pub fn start(
        searcher: Option<&Arc<dyn tools::HybridSearcher>>,
        store: &Arc<db::Store>,
        req: RecallRequest<'_>,
    ) -> RecallPrefetch {
        let (Some(searcher), false) = (searcher, req.prompt.trim().is_empty()) else {
            return RecallPrefetch { search: None };
        };
        let found = db_context::spawn_prompt_recall(searcher, req.user_id, req.prompt);
        let store = store.clone();
        let user_id = req.user_id.to_string();
        let prompt = req.prompt.to_string();
        let RecallRequest { tacit_only, skip, .. } = req;
        let search = tokio::spawn(async move {
            let results = db_context::recall_within_budget(found, &store, &user_id, &prompt).await;
            db_context::select_prompt_memories(results, &skip, tacit_only)
                .into_iter()
                .filter_map(|r| {
                    let memory = store.get_memory(r.memory_id?).ok().flatten()?;
                    Some(ScoredMemory { memory, score: r.score })
                })
                .take(MAX_RECALLED)
                .collect()
        });
        RecallPrefetch {
            search: Some(search),
        }
    }

    /// Called once per step, never waits. When the search has finished, its
    /// memories that `surfaced` does not already hold are queued as the
    /// `relevant_memories` attachment, added to `surfaced` and counted as
    /// accessed; returns whether anything was queued. Before the search
    /// finishes, and on every step after it landed, does nothing.
    pub fn land(
        &mut self,
        reminders: &mut Reminders,
        surfaced: &mut HashSet<i64>,
        store: &Arc<db::Store>,
    ) -> bool {
        if !self.search.as_ref().is_some_and(JoinHandle::is_finished) {
            return false;
        }
        let Some(search) = self.search.take() else {
            return false;
        };
        // Finished, so this resolves at once; a panicked search recalls nothing.
        let found = futures::FutureExt::now_or_never(search)
            .and_then(Result::ok)
            .unwrap_or_default();
        let fresh: Vec<ScoredMemory> = found
            .into_iter()
            .filter(|m| !surfaced.contains(&m.memory.id))
            .collect();
        if fresh.is_empty() {
            return false;
        }
        reminders.add(&TurnEvent::RelevantMemories(fresh.clone()));
        if !reminders::enabled(RELEVANT_MEMORIES) {
            return false;
        }
        let ids: Vec<i64> = fresh.iter().map(|m| m.memory.id).collect();
        surfaced.extend(&ids);
        record_access(store, ids);
        true
    }
}

impl Drop for RecallPrefetch {
    /// A turn that ends before the search finished stops it.
    fn drop(&mut self) {
        if let Some(search) = self.search.take() {
            search.abort();
        }
    }
}

/// The memories this conversation has already been shown: the ids on its
/// `relevant_memories` rows since the last checkpoint.
pub fn surfaced_memories(history: &[ChatMessage]) -> HashSet<i64> {
    history
        .iter()
        .filter_map(attachment_fields)
        .filter(|f| f.get("kind").and_then(|k| k.as_str()) == Some(RELEVANT_MEMORIES))
        .filter_map(|f| f.get("ids").and_then(|ids| ids.as_array()).cloned())
        .flatten()
        .filter_map(|id| id.as_i64())
        .collect()
}

/// Access accounting: memories that entered the turn's context (the
/// identity slice at Prepare, a recall when it lands) are counted as
/// accessed, so decay ranking reflects real use. Spawned: never blocks a
/// step.
pub fn record_access(store: &Arc<db::Store>, ids: Vec<i64>) {
    if ids.is_empty() {
        return;
    }
    let store = store.clone();
    tokio::spawn(async move {
        for id in ids {
            let _ = store.increment_memory_access(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::search_adapter::HybridSearchAdapter;
    use crate::session::SessionManager;

    /// A searcher that answers only once its gate opens (FTS underneath).
    struct Gated {
        inner: HybridSearchAdapter,
        gate: Arc<tokio::sync::Notify>,
    }

    impl tools::HybridSearcher for Gated {
        fn search<'a>(
            &'a self,
            query: &'a str,
            user_id: &'a str,
            limit: usize,
            min_score: Option<f64>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<tools::HybridSearchResult>> + Send + 'a>>
        {
            Box::pin(async move {
                self.gate.notified().await;
                self.inner.search(query, user_id, limit, min_score).await
            })
        }
    }

    struct Fixture {
        store: Arc<db::Store>,
        sessions: SessionManager,
        session_id: String,
    }

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nebo-recall-{}.db", uuid::Uuid::new_v4()));
            let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("test store"));
            let sessions = SessionManager::new(store.clone());
            let session_id = sessions.get_or_create("agent:a1:web", "").expect("session").id;
            // BM25 is corpus-relative: a handful of rows scores every match
            // under the unrequested-recall floor. Unrelated rows give the
            // words their weight, as a real store does.
            for i in 0..30 {
                store
                    .upsert_memory("tacit/general", &format!("filler/{i}"), &format!("Unrelated note number {i} about gardening"), None, None, "local")
                    .expect("filler");
            }
            Fixture {
                store,
                sessions,
                session_id,
            }
        }

        fn remember(&self, user_id: &str, key: &str, value: &str) -> i64 {
            self.store
                .upsert_memory("tacit/general", key, value, None, None, user_id)
                .expect("upsert");
            self.store
                .get_memory_by_key_and_user("tacit/general", key, user_id)
                .expect("read")
                .expect("row")
                .id
        }

        fn searcher(&self) -> Arc<dyn tools::HybridSearcher> {
            Arc::new(HybridSearchAdapter::new(self.store.clone(), None))
        }

        fn start(&self, searcher: &Arc<dyn tools::HybridSearcher>, user_id: &str, prompt: &str, skip: HashSet<i64>) -> RecallPrefetch {
            RecallPrefetch::start(
                Some(searcher),
                &self.store,
                RecallRequest {
                    prompt,
                    user_id,
                    tacit_only: false,
                    skip,
                },
            )
        }

        /// The conversation as the harness loads it: every row, attachment
        /// rows included.
        fn history(&self) -> Vec<ChatMessage> {
            let chat_id = self.sessions.active_chat_id(&self.session_id);
            self.store.get_chat_messages(&chat_id).expect("load")
        }

        fn access_count(&self, id: i64) -> i64 {
            self.store.get_memory(id).unwrap().unwrap().access_count.unwrap_or(0)
        }
    }

    /// Steps until the search has finished and been asked; how many steps
    /// found nothing, and whether the landing step queued an attachment.
    async fn step_until_finished(
        p: &mut RecallPrefetch,
        reminders: &mut Reminders,
        surfaced: &mut HashSet<i64>,
        store: &Arc<db::Store>,
    ) -> (u32, bool) {
        let mut empty_steps = 0;
        for _ in 0..400 {
            if p.land(reminders, surfaced, store) {
                return (empty_steps, true);
            }
            if p.search.is_none() {
                return (empty_steps, false);
            }
            empty_steps += 1;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the recall never finished");
    }

    fn written_ids(history: &[ChatMessage]) -> Vec<HashSet<i64>> {
        history
            .iter()
            .filter(|m| reminders::attachment_kind(m).as_deref() == Some(RELEVANT_MEMORIES))
            .map(|m| surfaced_memories(std::slice::from_ref(m)))
            .collect()
    }

    #[tokio::test]
    async fn recall_lands_on_first_ready_step() {
        let f = Fixture::new();
        let owner = "local:agent:a1";
        let id = f.remember(owner, "invoice/format", "Invoices go out as PDF with the logo");
        let gate = Arc::new(tokio::sync::Notify::new());
        let searcher: Arc<dyn tools::HybridSearcher> = Arc::new(Gated {
            inner: HybridSearchAdapter::new(f.store.clone(), None),
            gate: gate.clone(),
        });
        let mut p = f.start(&searcher, owner, "send the invoice to the client", HashSet::new());
        let mut reminders = Reminders::default();
        let mut surfaced = HashSet::new();

        // The search has not answered: the step goes on without it.
        assert!(!p.land(&mut reminders, &mut surfaced, &f.store), "a step never waits for the recall");
        reminders.write(&f.sessions, &f.session_id).unwrap();
        assert!(written_ids(&f.history()).is_empty(), "nothing is written before the search finishes");

        gate.notify_one();
        let (_, landed) = step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await;
        assert!(landed, "the first step after the search finished writes the attachment");
        reminders.write(&f.sessions, &f.session_id).unwrap();
        assert_eq!(written_ids(&f.history()), vec![HashSet::from([id])]);
        assert_eq!(surfaced, HashSet::from([id]));

        // Every later step asks nothing and writes nothing.
        assert!(!p.land(&mut reminders, &mut surfaced, &f.store));
        reminders.write(&f.sessions, &f.session_id).unwrap();
        assert_eq!(written_ids(&f.history()).len(), 1);

        // Access accounting: the landed memory was counted.
        for _ in 0..100 {
            if f.access_count(id) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(f.access_count(id), 1);
    }

    #[tokio::test]
    async fn recalled_memories_persist_on_later_steps() {
        let f = Fixture::new();
        let owner = "local:agent:a1";
        let id = f.remember(owner, "client/tone", "The client prefers short formal emails");
        let searcher = f.searcher();
        f.sessions.append_message(&f.session_id, "user", "draft an email to the client", None, None, None).unwrap();
        let mut p = f.start(&searcher, owner, "draft an email to the client", HashSet::new());
        let mut reminders = Reminders::default();
        let mut surfaced = HashSet::new();
        assert!(step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
        reminders.write(&f.sessions, &f.session_id).unwrap();

        // Later steps: the model replies, tools run; the row stays where it
        // was written and is loaded with the conversation every time.
        for i in 0..3 {
            f.sessions.append_message(&f.session_id, "assistant", &format!("step {i}"), None, None, None).unwrap();
            let history = f.history();
            let row = history
                .iter()
                .position(|m| reminders::attachment_kind(m).as_deref() == Some(RELEVANT_MEMORIES))
                .expect("the recall row is in the conversation");
            assert_eq!(row, 1, "right after the owner's message, never moved");
            assert!(history[row].content.contains("The client prefers short formal emails"));
            assert_eq!(surfaced_memories(&history), HashSet::from([id]));
        }
    }

    #[tokio::test]
    async fn surfaced_memory_not_repeated() {
        let f = Fixture::new();
        let owner = "local:agent:a1";
        let first = f.remember(owner, "invoice/format", "Invoices go out as PDF with the logo");
        let searcher = f.searcher();

        // Turn 1 surfaces the memory.
        let mut p = f.start(&searcher, owner, "send the invoice", HashSet::new());
        let mut reminders = Reminders::default();
        let mut surfaced = surfaced_memories(&f.history());
        assert!(step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
        reminders.write(&f.sessions, &f.session_id).unwrap();

        // Turn 2 asks again: the conversation already showed it, so the
        // recall writes nothing.
        let seen = surfaced_memories(&f.history());
        assert_eq!(seen, HashSet::from([first]));
        let mut p = f.start(&searcher, owner, "send the invoice", seen.clone());
        let mut surfaced = seen;
        assert!(!step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);

        // A new memory surfaces; the old one is not repeated beside it.
        let second = f.remember(owner, "invoice/due", "Invoices are due in 30 days");
        let seen = surfaced_memories(&f.history());
        let mut p = f.start(&searcher, owner, "send the invoice", seen.clone());
        let mut surfaced = seen;
        assert!(step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
        reminders.write(&f.sessions, &f.session_id).unwrap();
        assert_eq!(
            written_ids(&f.history()),
            vec![HashSet::from([first]), HashSet::from([second])]
        );

        // Even a prefetch started without the skip set never repeats what
        // the turn's state already holds.
        let mut p = f.start(&searcher, owner, "send the invoice", HashSet::new());
        let mut surfaced = surfaced_memories(&f.history());
        assert!(!step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
    }

    /// Moved from `memory.rs` (`test_memory_scope_chain_ctx_excludes_siblings`)
    /// and run through the recall itself: an isolated seat reads its own
    /// context, the agent scope and the owner, never a sibling context or a
    /// sibling agent.
    #[tokio::test]
    async fn isolated_scope_never_recalls_sibling_ctx() {
        let chain = crate::memory::memory_scope_chain("local:agent:a1:ctx:chat-A");
        assert_eq!(
            chain,
            vec![
                "local:agent:a1:ctx:chat-A".to_string(),
                "local:agent:a1".to_string(),
                "local".to_string(),
            ]
        );
        assert!(!chain.iter().any(|s| s.contains(":ctx:chat-B")));
        assert!(!chain.iter().any(|s| s.contains(":agent:a2")));

        let f = Fixture::new();
        let own = f.remember("local:agent:a1:ctx:chat-A", "case/deadline-a", "The filing deadline for this case is Friday");
        f.remember("local:agent:a1:ctx:chat-B", "case/deadline-b", "The filing deadline for the other case is Monday");
        f.remember("local:agent:a2", "case/deadline-c", "The filing deadline for another employee is Tuesday");
        let searcher = f.searcher();
        let mut p = f.start(&searcher, "local:agent:a1:ctx:chat-A", "what is the filing deadline", HashSet::new());
        let mut reminders = Reminders::default();
        let mut surfaced = HashSet::new();
        assert!(step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
        assert_eq!(surfaced, HashSet::from([own]), "only the seat's own context is recalled");
        reminders.write(&f.sessions, &f.session_id).unwrap();
        let text: String = f.history().iter().map(|m| m.content.clone()).collect();
        assert!(!text.contains("Monday") && !text.contains("Tuesday"), "{text}");
    }

    #[tokio::test]
    async fn restricted_audience_recalls_tacit_only() {
        let f = Fixture::new();
        let owner = "local:agent:a1";
        f.store
            .upsert_memory("project/matter", "matter/budget", "The budget for the matter is fixed", None, None, owner)
            .unwrap();
        let style = f.remember(owner, "style/budget", "Talk about the budget plainly");
        let searcher = f.searcher();
        let mut p = RecallPrefetch::start(
            Some(&searcher),
            &f.store,
            RecallRequest {
                prompt: "what is the budget for the matter",
                user_id: owner,
                tacit_only: true,
                skip: HashSet::new(),
            },
        );
        let mut reminders = Reminders::default();
        let mut surfaced = HashSet::new();
        assert!(step_until_finished(&mut p, &mut reminders, &mut surfaced, &f.store).await.1);
        assert_eq!(surfaced, HashSet::from([style]), "matter facts never reach a restricted audience");
    }

    #[test]
    fn nothing_to_search_never_lands() {
        let f = Fixture::new();
        let mut p = RecallPrefetch::start(
            None,
            &f.store,
            RecallRequest {
                prompt: "anything",
                user_id: "local",
                tacit_only: false,
                skip: HashSet::new(),
            },
        );
        assert!(p.search.is_none());
        assert!(!p.land(&mut Reminders::default(), &mut HashSet::new(), &f.store));
    }
}
