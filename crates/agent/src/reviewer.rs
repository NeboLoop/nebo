//! The reviewer: a second look at a run that is going in circles.
//!
//! The runner already tells a model when it repeats itself: the identical-call
//! note on the second call, the same-error nudge on the third, the duplicate-call
//! and serial-read steering reminders. A model that ignores all of that does not
//! need another note in the same voice; it needs a different reader with a
//! different context. So when a loop-class reminder has fired twice in one run,
//! a cheap model is shown the goal and the last few steps and asked for one
//! concrete piece of advice, or to say that continuing cannot help.
//!
//! Triggered, never resident: it costs nothing on a run that is fine, cannot
//! fire more than once per [`REVIEW_COOLDOWN_ITERATIONS`], and only ever
//! speaks (a stream reminder) or stops (a control notice). It never edits state.

use std::sync::Arc;

use ai::{ChatRequest, Message, Provider, StreamEventType};
use config::ModelsConfig;
use tracing::{debug, warn};

use crate::runner::truncate_str;

/// Steering reminders that mean "you are repeating yourself".
pub const LOOP_REMINDERS: &[&str] =
    &["duplicate_tool_call", "repetition_detector", "error_recovery", "serial_read_grind", "read_only_grind"];
/// Loop-class nudges in one run before the reviewer looks.
pub const REVIEW_AFTER_LOOP_NUDGES: usize = 2;
/// Iterations between two reviews of the same run.
pub const REVIEW_COOLDOWN_ITERATIONS: usize = 8;
/// Steps the reviewer is shown.
pub const RECENT_STEPS: usize = 10;
const STEP_TEXT_CHARS: usize = 240;
const STEP_IO_CHARS: usize = 160;
const GOAL_CHARS: usize = 600;
const MAX_TOKENS: i32 = 300;

const SYSTEM_PROMPT: &str = "You are reviewing another AI employee's recent steps on a job. It has \
been warned twice that it is repeating itself. Read the goal and the steps, then answer with JSON \
only, no prose: {\"advice\": \"<one or two concrete sentences: what to do next instead>\", \
\"stop\": <true only when continuing cannot help: the goal is impossible as stated, or it needs \
the owner to decide something>}. Be specific about what it already has and what it should do with it.";

/// Decides when a review is due. Counts loop-class nudges; the second one in a
/// run triggers a review, later ones only after the cooldown.
#[derive(Debug, Default)]
pub struct Trigger {
    loop_nudges: usize,
    last_review: Option<usize>,
}

impl Trigger {
    /// Record a fired reminder by name. True when the reviewer should look now.
    pub fn note(&mut self, reminder: &str, iteration: usize) -> bool {
        if !LOOP_REMINDERS.contains(&reminder) {
            return false;
        }
        self.loop_nudges += 1;
        if self.loop_nudges < REVIEW_AFTER_LOOP_NUDGES {
            return false;
        }
        if self.last_review.is_some_and(|last| iteration < last + REVIEW_COOLDOWN_ITERATIONS) {
            return false;
        }
        self.last_review = Some(iteration);
        true
    }
}

/// What the reviewer said.
/// Steps run on the escalation model after a stop verdict before the run
/// returns to its own model. Long enough to take one different path, short
/// enough that a wrong escalation cannot spend the run.
pub const ESCALATION_ITERATIONS: usize = 4;

/// The model a stopped run moves to: the configured "provider/model" spec
/// when its provider is connected. `None` means the stop stands, which is
/// also the answer for an empty spec or a provider that is not here.
pub fn escalation_model(spec: &str, providers: &[Arc<dyn Provider>]) -> Option<String> {
    let (provider_id, model) = spec.split_once('/')?;
    if model.is_empty() {
        return None;
    }
    providers.iter().any(|p| p.id() == provider_id).then(|| spec.to_string())
}

/// The model an open escalation window puts the next step on, if any.
pub fn window_model(window: Option<&(String, usize)>, iteration: usize) -> Option<&str> {
    window.filter(|(_, until)| iteration < *until).map(|(model, _)| model.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub advice: String,
    pub stop: bool,
}

/// Read a verdict out of the model's reply, tolerating prose around the JSON.
/// No advice means no verdict: the reviewer either says something or is ignored.
pub fn parse_verdict(text: &str) -> Option<Verdict> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let advice = v.get("advice")?.as_str()?.trim().to_string();
    if advice.is_empty() {
        return None;
    }
    Some(Verdict { advice, stop: v.get("stop").and_then(|s| s.as_bool()).unwrap_or(false) })
}

/// The last [`RECENT_STEPS`] steps as one line each: what the model said, what
/// it called, what came back. Oldest first.
pub fn describe_steps(messages: &[db::models::ChatMessage]) -> Vec<String> {
    let mut steps: Vec<String> = messages
        .iter()
        .rev()
        .filter(|m| m.role == "assistant" || m.role == "tool")
        .take(RECENT_STEPS)
        .map(|m| {
            if m.role == "tool" {
                let body = m.tool_results.as_deref().unwrap_or(&m.content);
                format!("result: {}", truncate_str(body, STEP_IO_CHARS))
            } else {
                let mut line = String::new();
                if !m.content.trim().is_empty() {
                    line.push_str(&format!("said: {}", truncate_str(m.content.trim(), STEP_TEXT_CHARS)));
                }
                if let Some(calls) = m.tool_calls.as_deref().filter(|c| !c.is_empty() && *c != "[]") {
                    if !line.is_empty() {
                        line.push_str(" | ");
                    }
                    line.push_str(&format!("called: {}", truncate_str(calls, STEP_IO_CHARS)));
                }
                line
            }
        })
        .filter(|l| !l.is_empty())
        .collect();
    steps.reverse();
    steps
}

/// The prompt the reviewer reads: goal, the detector's finding, the steps.
pub fn build_prompt(goal: &str, steps: &[String], finding: &str) -> String {
    let mut p = String::new();
    p.push_str("Goal: ");
    p.push_str(truncate_str(goal, GOAL_CHARS));
    p.push_str("\nWarned for: ");
    p.push_str(finding);
    p.push_str("\nRecent steps, oldest first:\n");
    for (i, s) in steps.iter().enumerate() {
        p.push_str(&format!("{}. {}\n", i + 1, s));
    }
    p
}

/// Ask the cheap model for a verdict. None when no provider answered or the
/// reply carried no advice; the run then simply continues.
pub async fn review(
    providers: &[Arc<dyn Provider>],
    goal: &str,
    steps: &[String],
    finding: &str,
) -> Option<Verdict> {
    let (provider, aux_model) = match crate::runner::resolve_aux(&ModelsConfig::load(), providers) {
        Some(routed) => routed,
        None => (crate::summarizer::pick_cheapest(providers)?, String::new()),
    };
    let req = ChatRequest {
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: build_prompt(goal, steps, finding),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: MAX_TOKENS,
        temperature: 0.0,
        system: SYSTEM_PROMPT.to_string(),
        static_system: String::new(),
        model: aux_model,
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
    };
    match provider.stream(&req).await {
        Ok(mut rx) => {
            let mut response = String::new();
            while let Some(event) = rx.recv().await {
                match event.event_type {
                    StreamEventType::Text => response.push_str(&event.text),
                    StreamEventType::Error => {
                        warn!(error = ?event.error, "reviewer call failed");
                        return None;
                    }
                    StreamEventType::Done => break,
                    _ => {}
                }
            }
            let verdict = parse_verdict(&response);
            debug!(?verdict, "reviewer verdict");
            verdict
        }
        Err(e) => {
            warn!(error = %e, "reviewer provider call failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only `id()` matters here.
    struct IdOnly(&'static str);

    #[async_trait::async_trait]
    impl Provider for IdOnly {
        fn id(&self) -> &str {
            self.0
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            Err(ai::ProviderError::Request("stub".into()))
        }
    }

    #[test]
    fn escalation_needs_a_spec_and_a_connected_provider() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(IdOnly("janus"))];
        assert_eq!(escalation_model("janus/nebo-1-high", &providers).as_deref(), Some("janus/nebo-1-high"));
        assert!(escalation_model("", &providers).is_none(), "empty spec: the stop stands");
        assert!(escalation_model("janus/", &providers).is_none(), "no model named");
        assert!(escalation_model("anthropic/claude-opus-4-6", &providers).is_none(), "provider not connected");
    }

    #[test]
    fn the_window_closes_at_its_iteration() {
        let w = Some(("janus/nebo-1-high".to_string(), 7usize));
        assert_eq!(window_model(w.as_ref(), 6), Some("janus/nebo-1-high"));
        assert_eq!(window_model(w.as_ref(), 7), None, "closed at the end iteration");
        assert_eq!(window_model(None, 0), None);
    }

    #[test]
    fn the_second_loop_nudge_brings_the_reviewer_then_the_cooldown_holds_it() {
        let mut t = Trigger::default();
        assert!(!t.note("budget_warning", 3), "not a loop-class reminder");
        assert!(!t.note("duplicate_tool_call", 4), "the first loop nudge is the model's chance");
        assert!(t.note("serial_read_grind", 6), "the second brings the reviewer");
        assert!(!t.note("error_recovery", 9), "inside the cooldown");
        assert!(t.note("error_recovery", 6 + REVIEW_COOLDOWN_ITERATIONS), "after the cooldown");
    }

    #[test]
    fn a_verdict_survives_prose_around_the_json_and_needs_advice() {
        let v = parse_verdict("Sure. {\"advice\": \"Write the docs from the files you already read.\", \"stop\": false} ok")
            .unwrap();
        assert_eq!(v.advice, "Write the docs from the files you already read.");
        assert!(!v.stop);
        let stop = parse_verdict("{\"advice\":\"Ask the owner which repo.\",\"stop\":true}").unwrap();
        assert!(stop.stop);
        assert!(parse_verdict("{\"stop\": true}").is_none(), "no advice, no verdict");
        assert!(parse_verdict("no json here").is_none());
    }

    #[test]
    fn the_prompt_carries_goal_finding_and_steps_in_order() {
        let steps = vec!["said: reading | called: [os cat a.go]".to_string(), "result: package a".to_string()];
        let p = build_prompt("document the project", &steps, "serial_read_grind");
        assert!(p.starts_with("Goal: document the project"));
        assert!(p.contains("Warned for: serial_read_grind"));
        assert!(p.find("1. said: reading").unwrap() < p.find("2. result: package a").unwrap());
    }

    #[test]
    fn steps_are_the_last_few_oldest_first() {
        let msg = |role: &str, content: &str, calls: Option<&str>| db::models::ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: calls.map(str::to_string),
            tool_results: None,
            token_estimate: None,
            html: None,
        };
        let mut msgs = vec![msg("user", "do it", None)];
        for i in 0..(RECENT_STEPS + 3) {
            msgs.push(msg("assistant", &format!("step {i}"), Some("[{\"name\":\"os\"}]")));
        }
        let steps = describe_steps(&msgs);
        assert_eq!(steps.len(), RECENT_STEPS);
        assert!(steps[0].starts_with("said: step 3"), "{}", steps[0]);
        assert!(steps[RECENT_STEPS - 1].starts_with(&format!("said: step {}", RECENT_STEPS + 2)));
        assert!(steps[0].contains("called: [{\"name\":\"os\"}]"));
    }
}
