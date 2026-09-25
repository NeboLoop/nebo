//! Admission: one turn per session. Input that arrives while a turn runs is
//! queued as a mid-turn row the running turn hears at its next step; input
//! that arrives after its last step starts the next turn.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// A turn in flight on one session. The runner admits ONE per session key: a
/// second request while it runs is appended to the session as the owner's next
/// message (the loop reloads history every iteration, so the model hears it at
/// its next step) and the caller gets a status line instead of a second worker
/// on the same job. Live 2026-09-03: four voice "status?" calls started four
/// more runs on one thread; they fought over one file for five minutes.
pub struct ActiveTurn {
    pub started: std::time::Instant,
    pub progress: RunProgress,
    /// The turn's cancel token: set means the owner stopped it and its loop
    /// is unwinding, so the slot frees in a moment.
    pub cancel_token: CancellationToken,
    /// Its loop has ended (`TurnGuard::close`): nothing reads the thread for
    /// it any more, and the slot frees in a moment.
    pub closing: bool,
}

pub type ActiveTurns = Arc<std::sync::Mutex<HashMap<String, ActiveTurn>>>;

/// Admit a turn on `session_key`, or say why not. Check and insert are one
/// step under the lock so two callers cannot both pass.
pub fn admit_turn(
    turns: &ActiveTurns,
    session_key: &str,
    progress: RunProgress,
    cancel_token: CancellationToken,
) -> Result<TurnGuard, String> {
    let mut map = turns.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(active) = map.get(session_key) {
        return Err(busy_status_line(active));
    }
    map.insert(
        session_key.to_string(),
        ActiveTurn { started: std::time::Instant::now(), progress, cancel_token, closing: false },
    );
    Ok(TurnGuard { turns: turns.clone(), session_key: session_key.to_string() })
}

/// What admission did with a turn's input.
pub enum Admission {
    /// The session was free: the caller runs the turn.
    Admitted(TurnGuard),
    /// A turn is running: the input went into it as a mid-turn row. `status`
    /// is the busy line the caller shows (typed `QUEUED_INTO_RUNNING_TURN`).
    Queued { status: String },
}

/// How long a new turn waits for a closing turn's slot, in steps.
const SLOT_WAIT_STEPS: usize = 100;
const SLOT_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(50);

/// Admit a turn on `session_key`, or hand its input to the turn running
/// there. `queue` writes the input as a mid-turn row; it runs under the
/// admission lock, so no row is written after the running turn's hand-off
/// check (`TurnGuard::close`): a row written before it is heard by that turn
/// or the one it hands off to, and input arriving after it waits here.
///
/// A turn that is closing (its loop ended, or the owner stopped it) will not
/// read a queued row, so the new turn waits for its slot, briefly, and runs
/// itself; past the wait the input is queued after all and the thread keeps
/// it for the next turn.
pub async fn admit_or_queue(
    turns: &ActiveTurns,
    session_key: &str,
    progress: RunProgress,
    cancel_token: CancellationToken,
    queue: impl FnOnce(),
) -> Admission {
    let mut queue = Some(queue);
    for step in 0..=SLOT_WAIT_STEPS {
        {
            let mut map = turns.lock().unwrap_or_else(|p| p.into_inner());
            match map.get(session_key) {
                None => {
                    map.insert(
                        session_key.to_string(),
                        ActiveTurn { started: std::time::Instant::now(), progress, cancel_token, closing: false },
                    );
                    return Admission::Admitted(TurnGuard { turns: turns.clone(), session_key: session_key.to_string() });
                }
                Some(active) if step == SLOT_WAIT_STEPS || !(active.closing || active.cancel_token.is_cancelled()) => {
                    let status = busy_status_line(active);
                    if let Some(queue) = queue.take() {
                        queue();
                    }
                    return Admission::Queued { status };
                }
                Some(_) => {}
            }
        }
        tokio::time::sleep(SLOT_WAIT_STEP).await;
    }
    unreachable!("the last step always admits or queues")
}

/// True when the turn holding `session_key` is on its way out — cancelled, or
/// its loop has ended — so the next message should wait for the slot rather
/// than be queued into a loop that will not read it.
pub fn turn_is_closing(turns: &ActiveTurns, session_key: &str) -> bool {
    turns
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(session_key)
        .is_some_and(|t| t.closing || t.cancel_token.is_cancelled())
}

/// The typed stop reason a busy session answers with. Consumers render it as
/// status (chat: a note under the message, spinner kept; voice: read aloud),
/// never as the employee's reply.
pub const QUEUED_INTO_RUNNING_TURN: &str = "queued_into_running_turn";

/// Releases the session when the run's task ends, however it ends.
pub struct TurnGuard {
    turns: ActiveTurns,
    session_key: String,
}

impl TurnGuard {
    /// The turn's loop has ended. The task still has its tail to run (the
    /// report, title, cleanup) before the slot frees; a message arriving now
    /// waits for the slot and starts the next turn instead of being queued
    /// into this one, which will not read it.
    pub fn close(&self) {
        if let Some(t) = self.turns.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&self.session_key) {
            t.closing = true;
        }
    }

    /// The closed turn found input that arrived before it closed and runs
    /// another turn for it: the slot is open to queued input again.
    pub fn reopen(&self) {
        if let Some(t) = self.turns.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&self.session_key) {
            t.closing = false;
        }
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        // Recover a poisoned lock: a panic elsewhere must not leave the session
        // marked busy, which would queue every later message forever.
        self.turns.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.session_key);
    }
}

/// ONE answer to "is a turn running on this session": the same map the
/// admission check uses, so callers that never register with the server's
/// run registry (voice, MCP) are seen too.
pub fn session_is_busy(turns: &ActiveTurns, session_key: &str) -> bool {
    live_session_under(turns, session_key).is_some()
}

/// The live session under `session_key`: the key itself, or an activity
/// session a workflow turn runs under (`<turn session>:<activity>::<n>`).
/// The engine holds a case turn's own session key; the runner marks the
/// activity's. Seen live: a reply that landed mid-turn was "not busy" by
/// exact match, deferred, and the turn closed the case without hearing it.
pub fn live_session_under(turns: &ActiveTurns, session_key: &str) -> Option<String> {
    let map = turns.lock().unwrap_or_else(|p| p.into_inner());
    if map.contains_key(session_key) {
        return Some(session_key.to_string());
    }
    let prefix = format!("{session_key}:");
    map.keys().find(|k| k.starts_with(&prefix)).cloned()
}

pub use types::api::ActiveTurnStatus;

pub fn active_turn_status(turns: &ActiveTurns, session_key: &str) -> Option<ActiveTurnStatus> {
    let map = turns.lock().unwrap_or_else(|p| p.into_inner());
    map.get(session_key).map(ActiveTurn::status)
}

impl ActiveTurn {
    fn status(&self) -> ActiveTurnStatus {
        ActiveTurnStatus {
            elapsed_secs: self.started.elapsed().as_secs(),
            tool_calls: self.progress.tool_call_count.load(std::sync::atomic::Ordering::Relaxed),
            current_tool: self.progress.current_tool.lock().map(|t| t.clone()).unwrap_or_default(),
        }
    }
}

/// The live counters as one phrase ("3 minutes in, 12 tool calls so far,
/// currently running os: exec"). The busy line below and voice's `status`
/// tool both read it, so they never describe the same run differently.
pub fn progress_phrase(st: &ActiveTurnStatus) -> String {
    let elapsed = if st.elapsed_secs < 90 {
        format!("{} seconds", st.elapsed_secs)
    } else {
        format!("{} minutes", st.elapsed_secs / 60)
    };
    let doing = if st.current_tool.is_empty() {
        "thinking".to_string()
    } else {
        format!("running {}", st.current_tool)
    };
    let calls_part = match st.tool_calls {
        0 => String::new(),
        1 => ", 1 tool call so far".to_string(),
        n => format!(", {n} tool calls so far"),
    };
    format!("{elapsed} in{calls_part}, currently {doing}")
}

/// What a second caller hears while a turn is busy. Built from the live
/// counters, no model call; read aloud by voice, shown as status in chat.
pub fn busy_status_line(active: &ActiveTurn) -> String {
    format!(
        "Still on the last thing, {}. I'll pick this up at my next step; if that work \
         finishes first, your message is waiting in the thread.",
        progress_phrase(&active.status())
    )
}

/// Shared atomic counters for live run progress reporting.
/// Created by the server's RunRegistry and threaded into the runner.
#[derive(Clone, Debug)]
pub struct RunProgress {
    pub run_id: String,
    pub iteration_count: Arc<std::sync::atomic::AtomicU32>,
    pub tool_call_count: Arc<std::sync::atomic::AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress() -> RunProgress {
        RunProgress {
            run_id: "r".into(),
            iteration_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            tool_call_count: Arc::new(std::sync::atomic::AtomicU32::new(3)),
            current_tool: Arc::new(std::sync::Mutex::new("os: exec".into())),
        }
    }

    /// The live failure: a second request on a busy session must not become a
    /// second worker. It is refused with a status line, and the session opens
    /// again the moment the first turn's guard drops.
    #[test]
    fn one_turn_per_session_and_the_guard_reopens_it() {
        let turns: ActiveTurns = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let first = admit_turn(&turns, "agent:a:thread:t", progress(), CancellationToken::new()).expect("first turn admitted");
        let second = admit_turn(&turns, "agent:a:thread:t", progress(), CancellationToken::new());
        let status = match second {
            Err(s) => s,
            Ok(_) => panic!("a second turn was admitted on a busy session"),
        };
        assert!(status.contains("3 tool calls") && status.contains("running os: exec"), "{status}");
        assert!(!status.contains('\u{2014}'), "no em dash in owner copy");
        assert!(!status.contains("stop") && !status.contains("will answer"), "promises only what the code does: {status}");
        assert!(session_is_busy(&turns, "agent:a:thread:t"));
        let st = active_turn_status(&turns, "agent:a:thread:t").expect("status while busy");
        assert_eq!((st.tool_calls, st.current_tool.as_str()), (3, "os: exec"));
        assert!(active_turn_status(&turns, "agent:a:thread:other").is_none());
        assert!(admit_turn(&turns, "agent:a:thread:other", progress(), CancellationToken::new()).is_ok(), "other sessions are unaffected");
        drop(first);
        assert!(!session_is_busy(&turns, "agent:a:thread:t"));
        assert!(admit_turn(&turns, "agent:a:thread:t", progress(), CancellationToken::new()).is_ok(), "released when the guard drops");
    }

    /// The engine knows a case turn by its own session; the harness marks
    /// the activity session under it. The live session under a key is the
    /// key itself or an activity beneath it — never a key that merely
    /// shares a prefix.
    #[test]
    fn the_live_session_under_a_turn_key_is_its_activity_session() {
        let turns: ActiveTurns = Default::default();
        let activity = "agent:a:workflow:t1:capture::0";
        let _guard = admit_turn(&turns, activity, progress(), CancellationToken::new()).unwrap();
        assert_eq!(live_session_under(&turns, "agent:a:workflow:t1").as_deref(), Some(activity));
        assert_eq!(live_session_under(&turns, activity).as_deref(), Some(activity), "the key itself");
        assert_eq!(live_session_under(&turns, "agent:a:workflow:t"), None, "a shared prefix is not a session under it");
        assert!(session_is_busy(&turns, "agent:a:workflow:t1"), "busy by the turn's key");
    }

    /// A turn whose loop has ended is closing: a message arriving then waits
    /// for the slot and starts the next turn instead of being queued into a
    /// loop that will not read it.
    #[test]
    fn a_turn_whose_loop_ended_is_closing() {
        let turns: ActiveTurns = Default::default();
        let guard = admit_turn(&turns, "subagent:p:sa-1", progress(), CancellationToken::new()).unwrap();
        assert!(!turn_is_closing(&turns, "subagent:p:sa-1"), "running");
        guard.close();
        assert!(turn_is_closing(&turns, "subagent:p:sa-1"), "loop ended");
        assert!(session_is_busy(&turns, "subagent:p:sa-1"), "still holds the slot until its task ends");
        drop(guard);
        assert!(!session_is_busy(&turns, "subagent:p:sa-1"));
        let cancel = CancellationToken::new();
        let _g = admit_turn(&turns, "k", progress(), cancel.clone()).unwrap();
        cancel.cancel();
        assert!(turn_is_closing(&turns, "k"), "a stopped turn is closing too");
    }

    /// Input for a busy session goes into the running turn and the caller
    /// gets the busy line; a closing turn is waited for and the new turn
    /// runs itself, its input never written into the closing one.
    #[tokio::test]
    async fn busy_input_is_queued_and_a_closing_turn_is_waited_for() {
        let turns: ActiveTurns = Default::default();
        let running = admit_turn(&turns, "k", progress(), CancellationToken::new()).unwrap();
        let queued = std::sync::atomic::AtomicBool::new(false);
        let admission = admit_or_queue(&turns, "k", progress(), CancellationToken::new(), || {
            queued.store(true, std::sync::atomic::Ordering::SeqCst)
        })
        .await;
        assert!(matches!(admission, Admission::Queued { ref status } if status.contains("3 tool calls")));
        assert!(queued.load(std::sync::atomic::Ordering::SeqCst), "the input was written into the running turn");

        running.close();
        let released = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            drop(running);
        });
        let admission = admit_or_queue(&turns, "k", progress(), CancellationToken::new(), || {
            panic!("input queued into a closing turn")
        })
        .await;
        assert!(matches!(admission, Admission::Admitted(_)), "the new turn runs once the slot frees");
        released.await.unwrap();
    }

    /// A closed turn that found input waiting reopens: input arriving then
    /// is queued into the turn it hands off to, not left for a later one.
    #[test]
    fn a_reopened_turn_takes_queued_input_again() {
        let turns: ActiveTurns = Default::default();
        let guard = admit_turn(&turns, "k", progress(), CancellationToken::new()).unwrap();
        guard.close();
        assert!(turn_is_closing(&turns, "k"));
        guard.reopen();
        assert!(!turn_is_closing(&turns, "k"));
    }
}
