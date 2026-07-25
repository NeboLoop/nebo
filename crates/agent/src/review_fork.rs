//! Self-improvement review fork — trigger bookkeeping + prompt.
//!
//! After a turn on an employee with `learning_mode = "auto"`, the runner may
//! fork the conversation (`fork:<session>:review-<uuid>` session) and ask
//! "should any skill be learned or updated?". Writes land in the per-employee
//! LEARNED tree via the skill tool's fork-only pathway (`learned_write_agent`
//! on `ToolContext`), restricted by a dispatch-time whitelist and
//! read-before-write marks. Design: docs/design/SELF_IMPROVEMENT.md (WS2),
//! mechanism study: docs/design/SELF_IMPROVEMENT_STUDY.md.
//!
//! This module owns the trigger counters (turns since the last VOLUNTARY
//! skill save — a review fires only when organic learning stalled) and the
//! per-session single-flight guard (a gap Hermes left open). The spawn itself
//! lives in `runner::run_loop`'s post-turn tail beside memory extraction,
//! where the full run context is in scope.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Turns without a voluntary skill save before a review fires (Hermes
/// default). A voluntary `skill create/update` during a normal run resets
/// the counter — the review is a backstop, not a metronome.
pub const REVIEW_TURN_INTERVAL: u32 = 10;

/// Max iterations for the fork run — enough to load + update a couple of
/// skills, far below a runaway loop.
pub const REVIEW_MAX_ITERATIONS: usize = 12;

/// Restrictions a review-fork run carries into `run_loop` → `ToolContext`:
/// the owning employee (learned-write target + read scope), the dispatch
/// whitelist, and the shared read-marks set for read-before-write.
#[derive(Clone)]
pub struct ReviewForkCtx {
    pub owner_agent_id: String,
    pub whitelist: HashSet<String>,
    pub skills_read: std::sync::Arc<Mutex<HashSet<String>>>,
    /// learning_mode == "staged": learned-skill writes go to pending_writes
    /// for Inbox approval instead of landing on disk.
    pub staged: bool,
}

impl ReviewForkCtx {
    pub fn new(owner_agent_id: String, staged: bool) -> Self {
        Self {
            owner_agent_id,
            whitelist: HashSet::from(["skill".to_string()]),
            skills_read: std::sync::Arc::new(Mutex::new(HashSet::new())),
            staged,
        }
    }
}

static COUNTERS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
static IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Count this turn for `session_id`; true when the review threshold is hit
/// (counter resets). Call once per completed turn.
pub fn should_review(session_id: &str) -> bool {
    let mut guard = COUNTERS.lock().unwrap_or_else(|p| p.into_inner());
    let counters = guard.get_or_insert_with(HashMap::new);
    let count = counters.entry(session_id.to_string()).or_insert(0);
    *count += 1;
    if *count >= REVIEW_TURN_INTERVAL {
        *count = 0;
        true
    } else {
        false
    }
}

/// The model saved a skill on its own this turn — organic learning is
/// happening, push the backstop out.
pub fn note_voluntary_save(session_id: &str) {
    let mut guard = COUNTERS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(counters) = guard.as_mut() {
        counters.insert(session_id.to_string(), 0);
    }
}

/// Single-flight: returns true if no review is currently running for this
/// session (and marks one running). Pair with `finish`.
pub fn try_begin(session_id: &str) -> bool {
    let mut guard = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .get_or_insert_with(HashSet::new)
        .insert(session_id.to_string())
}

/// Mark the session's review finished (success or failure).
pub fn finish(session_id: &str) {
    let mut guard = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(set) = guard.as_mut() {
        set.remove(session_id);
    }
}

/// The review prompt — Hermes `_SKILL_REVIEW_PROMPT` adapted to Nebo's skill
/// tool (load / update / create; memory is handled by the separate extraction
/// pass, not this fork). Keep the ACTIVE bias, the update ladder, and the
/// anti-capture rules intact — they are the loop's quality control.
pub const REVIEW_PROMPT: &str = "\
Review the conversation above and update your skill library. Be ACTIVE — \
most sessions produce at least one skill update, even if small. A pass that \
does nothing is a missed learning opportunity, not a neutral outcome.\n\n\
You are updating LEARNED skills: private lessons about how to do this class \
of task for this owner. Marketplace and user-authored skills are read-only \
to you. Target shape: CLASS-LEVEL skills with rich bodies — not a flat list \
of narrow one-session entries.\n\n\
Signals to look for (any one warrants action):\n\
- The user corrected your style, tone, format, or verbosity. Frustration \
signals like 'stop doing X', 'this is too verbose', 'don't format like \
this', or an explicit 'remember this' are FIRST-CLASS skill signals. Embed \
the preference so the next session starts already knowing.\n\
- The user corrected your workflow, approach, or sequence of steps. Encode \
the correction as a pitfall or explicit step in the skill governing that \
class of task.\n\
- A non-trivial technique, fix, workaround, or debugging path emerged that a \
future session would benefit from.\n\
- A skill you loaded this session turned out wrong, missing a step, or \
outdated — patch it NOW.\n\n\
HOW to save — follow these steps IN ORDER, in this pass, right now. (You \
list and load skills YOURSELF here; nothing needs to have been loaded \
earlier in the conversation.)\n\
1. skill(action: \"list\") — see what learned skills already exist.\n\
2. If one covers this class of task: skill(action: \"load\", name: \"...\") \
to read its CURRENT content, then skill(action: \"update\", name: \"...\", \
content: \"<full revised SKILL.md>\") with the new lesson merged in. \
Updates are refused unless you load the skill first IN THIS PASS — that is \
the required order, not a blocker.\n\
3. Only if NO existing skill covers it: skill(action: \"create\", name: \
\"...\", content: \"---\\nname: ...\\ndescription: <trigger class, under 60 \
chars>\\n---\\n<body>\"). The name MUST describe a class of work (e.g. \
'report-formatting', 'vendor-email-tone') — NEVER a specific error string, \
ticket, or 'fix-X-today' session artifact.\n\n\
User-preference embedding (important): memory captures WHO the owner is; \
LEARNED skills capture HOW to do a class of task for this owner. A \
preference that was stored to memory during the conversation does NOT count \
as captured — memory alone is not enough. When the owner corrected how you \
handle a class of task (email tone, report format, reply length), the \
learned skill governing that class of task must carry the lesson too, so \
the next session starts already knowing. 'It's already in memory' is never \
a reason to skip the skill update.\n\n\
Do NOT capture (these become self-imposed constraints that bite later):\n\
- Environment-dependent failures: missing binaries, 'command not found', \
unconfigured credentials, uninstalled packages. If a tool failed because of \
setup state, capture the FIX (install command, config step) under a setup \
lesson — never 'this tool does not work' as a standalone rule.\n\
- Negative claims about tools or features ('X is broken', 'cannot use Y'). \
These harden into refusals cited long after the problem was fixed.\n\
- Transient errors that resolved before the conversation ended. If retrying \
worked, the lesson is the retry pattern, not the failure.\n\
- One-off task narratives. A single 'summarize this report' request is not \
a class of work that warrants a skill.\n\n\
DECISION RULE — apply it mechanically before replying:\n\
Scan the conversation for any of: (a) an explicit 'remember this' / 'from \
now on' instruction, (b) a style, format, tone, or length correction, (c) a \
workflow or approach correction, (d) a reusable technique or fix.\n\
- If ANY are present: you MUST run the HOW steps above (list → load+update \
or create) BEFORE replying. Replying 'Nothing to save.' while a correction \
exists in the conversation is a failure of this pass. Not-an-excuse list: \
'it's already in memory' (memory is not the skill library), 'no skill was \
loaded during the conversation' (you load it yourself in this pass), 'the \
preference is minor' (corrections are always saved).\n\
- Only if NONE are present: reply exactly 'Nothing to save.' and stop.\n\
After saving, reply with one short line per skill created or updated.\n\n\
Only the skill tool is available in this pass; other tools will be denied.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_fires_every_interval_and_resets() {
        let sid = format!("test-review-{}", uuid::Uuid::new_v4());
        for _ in 0..REVIEW_TURN_INTERVAL - 1 {
            assert!(!should_review(&sid));
        }
        assert!(should_review(&sid));
        assert!(!should_review(&sid), "counter must reset after firing");
    }

    #[test]
    fn voluntary_save_resets_counter() {
        let sid = format!("test-voluntary-{}", uuid::Uuid::new_v4());
        for _ in 0..REVIEW_TURN_INTERVAL - 1 {
            should_review(&sid);
        }
        note_voluntary_save(&sid);
        assert!(
            !should_review(&sid),
            "turn after a voluntary save must not trigger"
        );
    }

    #[test]
    fn single_flight_guards_concurrent_reviews() {
        let sid = format!("test-flight-{}", uuid::Uuid::new_v4());
        assert!(try_begin(&sid));
        assert!(!try_begin(&sid), "second begin must be refused");
        finish(&sid);
        assert!(try_begin(&sid), "finish must release the slot");
        finish(&sid);
    }
}
