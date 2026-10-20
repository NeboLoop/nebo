//! Collecting a helper's result: its final message only (or its last words,
//! marked, when it stopped without one, or its question for the owner). A
//! report past its limit goes through the one spill path: the full text
//! saved, a preview of its start and the file's path inline.

use std::path::Path;
use std::time::Duration;

use ai::{StreamEvent, StreamEventType};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Completion, CompletionStatus};

/// The default length of a report its parent reads inline
/// (`tools::orchestrator::REPORT_LIMIT`); a longer one is saved and previewed.
pub use tools::orchestrator::REPORT_LIMIT;

/// Why a helper that asked for the owner's decision stopped
/// ([`CompletionStatus::Partial`]).
pub const NEEDS_OWNER_DECISION: &str = "needs owner decision";

/// The `stop_reason` of a helper turn's `Done` event when the turn ended at
/// its step limit (`TurnExit::MaxSteps`). The turn driver sets it.
pub const STOP_MAX_STEPS: &str = "max_steps";
/// The `stop_reason` of the `Done` event when the turn ended at its spending
/// limit (`TurnExit::SpendCap`).
pub const STOP_SPEND_CAP: &str = "spend_cap";
/// The `stop_reason` of the `Done` event when the turn saw its cancel and
/// stopped (`TurnExit::Cancelled`): the helper was stopped, whichever of the
/// cancel and this event the collector wakes to first.
pub const STOP_CANCELLED: &str = "cancelled";

/// Marks a result that is not a final report.
pub const NO_FINAL_REPORT: &str = "[no final report; last words before it stopped]";

/// What a helper's turn produced.
#[derive(Debug, Default)]
pub struct Collected {
    /// The text after the helper's last tool call: its report.
    pub final_message: String,
    /// The last non-empty text before that, for a turn that ended without one.
    pub last_words: String,
    pub error: Option<String>,
    pub stalled: Option<Duration>,
    pub cancelled: bool,
    /// The turn ended at a limit, not because the work was done.
    pub limit: Option<&'static str>,
    pub usage: ai::UsageInfo,
    /// The untrusted content the turn read (its `Done` event's provenance).
    pub taint: Vec<types::provenance::ProvenanceClass>,
    /// The turn ended on a decision only the owner can make (`ask_owner`).
    pub question: Option<types::OwnerQuestion>,
}

impl Collected {
    pub fn failed(error: String) -> Self {
        Self { error: Some(error), ..Self::default() }
    }

    /// The completion this turn reports. A result over `report_limit`
    /// characters is saved in full in `spill_dir`, and its start and the
    /// file's path are what the parent reads.
    pub fn into_completion(self, task_id: &str, description: &str, spill_dir: &Path, report_limit: usize) -> Completion {
        let report = self.final_message.trim();
        let (text, status) = if let Some(error) = self.error {
            (self.last_words.trim().to_string(), CompletionStatus::Failed { error })
        } else if self.cancelled {
            (self.last_words.trim().to_string(), CompletionStatus::Stopped)
        } else if let Some(window) = self.stalled {
            (
                best_words(report, &self.last_words),
                CompletionStatus::Partial { why: format!("no activity for {}s", window.as_secs()) },
            )
        } else if let Some(why) = self.limit {
            (best_words(report, &self.last_words), CompletionStatus::Partial { why: why.to_string() })
        } else if let Some(question) = &self.question {
            (
                for_the_owner(task_id, question, &self.last_words),
                CompletionStatus::Partial { why: NEEDS_OWNER_DECISION.to_string() },
            )
        } else if report.is_empty() {
            (
                marked(&self.last_words),
                CompletionStatus::Partial { why: "it stopped without a final report".to_string() },
            )
        } else {
            (report.to_string(), CompletionStatus::Done)
        };
        Completion {
            task_id: task_id.to_string(),
            description: description.to_string(),
            status,
            result: spill_if_long(&text, spill_dir, report_limit),
            usage: self.usage,
            taint: self.taint,
        }
    }
}

fn marked(last_words: &str) -> String {
    let words = last_words.trim();
    if words.is_empty() {
        format!("{NO_FINAL_REPORT}\n(it wrote nothing)")
    } else {
        format!("{NO_FINAL_REPORT}\n{words}")
    }
}

fn best_words(report: &str, last_words: &str) -> String {
    if report.is_empty() { marked(last_words) } else { report.to_string() }
}

/// The report of a helper that stopped on a decision only the owner can
/// make: where it stands, the question, its options with the recommended
/// one, and how its parent gets the answer to it.
fn for_the_owner(task_id: &str, q: &types::OwnerQuestion, last_words: &str) -> String {
    let mut out = String::new();
    let words = last_words.trim();
    if !words.is_empty() {
        out.push_str(words);
        out.push_str("\n\n");
    }
    out.push_str(&format!("It needs the owner's decision before it can go on.\nQuestion: {}", q.question.trim()));
    if !q.context.trim().is_empty() {
        out.push_str(&format!("\nContext: {}", q.context.trim()));
    }
    if !q.options.is_empty() {
        out.push_str(&format!("\nOptions: {}", q.options.join(" | ")));
    }
    if let Some(recommended) = q.recommended() {
        out.push_str(&format!("\nRecommended: {recommended}"));
    }
    out.push_str(&format!(
        "\nUnless the owner already settled this in your conversation, ask them with ask_owner (this question, \
         context and options, the recommended one first). Then send_message the answer to {task_id}: it continues \
         from where it stopped."
    ));
    out
}

/// Read a helper turn's events to the end. `on_event` sees every event (the
/// owner's progress line is built from it); nothing is forwarded to the
/// parent. The turn ends at `Done`, at a closed stream, when `cancel`
/// fires, or after `inactivity` without an event while none of its calls
/// is waiting (`waiting`, the turn's `ToolContext::waiting`): a clip that
/// renders for ten minutes is not silence (audit 2026-10-07: shot helpers
/// were ended as stalled just as their clips landed).
pub async fn collect(
    mut events: mpsc::Receiver<StreamEvent>,
    cancel: &CancellationToken,
    inactivity: Duration,
    waiting: &tools::Waiting,
    mut on_event: impl FnMut(&StreamEvent),
) -> Collected {
    use crate::guardrails::{Next, next_event};
    let mut out = Collected::default();
    let mut current = String::new();
    let mut last_event = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                out.cancelled = true;
                break;
            }
            next = next_event(&mut events, last_event, waiting, inactivity) => {
                let event = match next {
                    Next::Event(event) => event,
                    Next::Closed => break,
                    Next::Stalled => {
                        cancel.cancel();
                        out.stalled = Some(inactivity);
                        break;
                    }
                };
                last_event = tokio::time::Instant::now();
                on_event(&event);
                match event.event_type {
                    StreamEventType::Text => current.push_str(&event.text),
                    StreamEventType::ToolCall => {
                        if !current.trim().is_empty() {
                            out.last_words = std::mem::take(&mut current);
                        }
                        current.clear();
                    }
                    StreamEventType::Usage => {
                        if let Some(u) = &event.usage {
                            add_usage(&mut out.usage, u);
                        }
                    }
                    StreamEventType::ControlNotice => {
                        if let Some(question) = event.owner_question() {
                            out.question = Some(question);
                        }
                    }
                    StreamEventType::Error => {
                        out.error = Some(event.error.unwrap_or_else(|| "the helper's turn failed".to_string()));
                    }
                    StreamEventType::Done => {
                        out.taint = event.provenance.clone().unwrap_or_default();
                        out.cancelled = event.stop_reason.as_deref() == Some(STOP_CANCELLED);
                        out.limit = match event.stop_reason.as_deref() {
                            Some(STOP_MAX_STEPS) => Some("hit its step limit"),
                            Some(STOP_SPEND_CAP) => Some("hit its spending limit"),
                            _ => None,
                        };
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    if !current.trim().is_empty() && !out.cancelled && out.stalled.is_none() {
        out.final_message = current;
    } else if !current.trim().is_empty() {
        out.last_words = current;
    }
    out
}

fn add_usage(total: &mut ai::UsageInfo, u: &ai::UsageInfo) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_creation_input_tokens += u.cache_creation_input_tokens;
    total.cache_read_input_tokens += u.cache_read_input_tokens;
    if let Some(c) = u.cost_microdollars {
        *total.cost_microdollars.get_or_insert(0) += c;
    }
}

/// `text`, or past `limit` characters the one spill path's note: the full
/// text saved in `spill_dir`, a preview of its start and the file's path
/// inline.
pub fn spill_if_long(text: &str, spill_dir: &Path, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    tools::result_shape::persist(spill_dir, text, tools::result_shape::Reader::File)
}

/// The first `max` characters of `s`, cut on a character boundary.
pub fn clip_chars(s: &str, max: usize) -> Option<&str> {
    s.char_indices().nth(max).map(|(i, _)| &s[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call() -> StreamEvent {
        StreamEvent::tool_call(ai::ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "a.txt"}),
        })
    }

    async fn run(events: Vec<StreamEvent>) -> Collected {
        let (tx, rx) = mpsc::channel(64);
        for e in events {
            tx.send(e).await.unwrap();
        }
        drop(tx);
        collect(rx, &CancellationToken::new(), Duration::from_secs(5), &tools::Waiting::default(), |_| {}).await
    }

    #[tokio::test]
    async fn only_final_message_returns_tail_kept() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![
            StreamEvent::text("Let me look at the file."),
            tool_call(),
            StreamEvent::text("Still looking."),
            tool_call(),
            StreamEvent::text("The invoice is "),
            StreamEvent::text("missing from March."),
            StreamEvent::done(),
        ])
        .await;
        let c = got.into_completion("h-1", "find it", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Done);
        assert_eq!(c.result, "The invoice is missing from March.", "narration never returns");

        // Oversized: the one spill path, as for any tool result: the whole
        // text saved, its start previewed.
        let long = format!("THE START{}THE END", "é".repeat(REPORT_LIMIT + 50));
        let got = run(vec![tool_call(), StreamEvent::text(long.clone()), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "find it", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Done);
        assert!(c.result.starts_with(tools::result_shape::SAVED_OUTPUT), "{}", &c.result[..200]);
        assert!(c.result.contains("THE START"));
        assert!(!c.result.contains("THE END"));
        let path = c.result.split("Saved in full at: ").nth(1).unwrap().lines().next().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), long, "the full text is on disk");
        assert!(c.result.chars().count() < 3_000);
    }

    /// A report keeps to the limit its delegate call set: past it, the full
    /// text is in a file and the parent reads its start and the path. Under
    /// it, the report comes back whole. The default is lean.
    #[tokio::test]
    async fn a_report_keeps_to_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let report = format!("30 listings checked.{}THE LAST LISTING", " row".repeat(1_000));
        let got = run(vec![tool_call(), StreamEvent::text(report.clone()), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "listings", dir.path(), 2_000);
        assert_eq!(c.status, CompletionStatus::Done, "a long report is still done");
        assert!(c.result.starts_with(tools::result_shape::SAVED_OUTPUT), "{}", c.result);
        assert!(c.result.contains("30 listings checked.") && !c.result.contains("THE LAST LISTING"));
        let path = c.result.split("Saved in full at: ").nth(1).unwrap().lines().next().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), report, "the full report is on disk");

        let got = run(vec![tool_call(), StreamEvent::text(report.clone()), StreamEvent::done()]).await;
        assert_eq!(got.into_completion("h-1", "listings", dir.path(), 10_000).result, report, "a raised limit keeps it whole");
        assert!(REPORT_LIMIT <= 10_000, "the default is lean: {REPORT_LIMIT}");
    }

    /// A helper can't reach the owner: its `ask_owner` ends its turn on the
    /// question, and the report says where it stands, the question, the
    /// options with the recommended one, and how its parent answers it.
    #[tokio::test]
    async fn a_question_for_the_owner_comes_back_in_the_report() {
        let dir = tempfile::tempdir().unwrap();
        let q = types::OwnerQuestion {
            question: "Which vendor should the outreach go to?".into(),
            context: "Two quotes came in under budget.".into(),
            options: vec!["Acme".into(), "Globex".into()],
        };
        let got = run(vec![
            StreamEvent::text("Drafted 18 of 20 emails."),
            tool_call(),
            StreamEvent::owner_decision("Your question goes back with your report.", &q),
            StreamEvent::done_with_reason("terminal_tool_error"),
        ])
        .await;
        let c = got.into_completion("h-7", "outreach batch", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Partial { why: NEEDS_OWNER_DECISION.into() });
        for part in [
            "Drafted 18 of 20 emails.",
            "Question: Which vendor should the outreach go to?",
            "Context: Two quotes came in under budget.",
            "Options: Acme | Globex",
            "Recommended: Acme",
            "ask them with ask_owner",
            "send_message the answer to h-7",
        ] {
            assert!(c.result.contains(part), "{part:?} missing from:\n{}", c.result);
        }

        // Any other terminal notice is no question.
        let got = run(vec![
            tool_call(),
            StreamEvent::control_notice("Gmail isn't connected.", "terminal_tool_error"),
            StreamEvent::done_with_reason("terminal_tool_error"),
        ])
        .await;
        assert!(got.question.is_none());
    }

    /// A turn that ended at its step or spending limit is partial, whatever
    /// it last said.
    #[tokio::test]
    async fn a_helper_at_its_limit_reports_partial() {
        let dir = tempfile::tempdir().unwrap();
        for (reason, why) in [(STOP_MAX_STEPS, "hit its step limit"), (STOP_SPEND_CAP, "hit its spending limit")] {
            let got = run(vec![tool_call(), StreamEvent::text("Got through March."), StreamEvent::done_with_reason(reason)]).await;
            let c = got.into_completion("h-1", "find it", dir.path(), REPORT_LIMIT);
            assert_eq!(c.status, CompletionStatus::Partial { why: why.into() });
            assert_eq!(c.result, "Got through March.");
        }
    }

    #[tokio::test]
    async fn no_final_report_returns_marked_last_words_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![StreamEvent::text("Checked March, now April."), tool_call(), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "find it", dir.path(), REPORT_LIMIT);
        assert!(matches!(c.status, CompletionStatus::Partial { .. }));
        assert_eq!(c.result, format!("{NO_FINAL_REPORT}\nChecked March, now April."));
    }

    #[tokio::test]
    async fn an_error_fails_and_a_cancel_stops() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![StreamEvent::text("x"), StreamEvent::error("provider down")]).await;
        let c = got.into_completion("h-1", "d", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Failed { error: "provider down".into() });

        let (_tx, rx) = mpsc::channel::<StreamEvent>(1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let got = collect(rx, &cancel, Duration::from_secs(5), &tools::Waiting::default(), |_| {}).await;
        assert_eq!(got.into_completion("h-1", "d", dir.path(), REPORT_LIMIT).status, CompletionStatus::Stopped);
    }

    /// A turn that saw its cancel ends with `Done` saying so. The collector
    /// can read that event before it wakes to the cancel itself (both are
    /// ready, one is picked): the helper was stopped either way, never
    /// completed with a partial report its parent is woken for.
    #[tokio::test]
    async fn a_turn_that_ended_on_its_cancel_is_stopped_whichever_arrives_first() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![tool_call(), StreamEvent::text("Got through March."), StreamEvent::done_with_reason(STOP_CANCELLED)]).await;
        let c = got.into_completion("h-1", "find it", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Stopped);
        assert_eq!(c.result, "Got through March.", "its last words, not a report");
    }

    #[tokio::test(start_paused = true)]
    async fn silence_ends_the_turn_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(4);
        tx.send(StreamEvent::text("Half way.")).await.unwrap();
        let cancel = CancellationToken::new();
        let got = collect(rx, &cancel, Duration::from_secs(600), &tools::Waiting::default(), |_| {}).await;
        assert!(cancel.is_cancelled(), "a stalled helper is ended");
        let c = got.into_completion("h-1", "d", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Partial { why: "no activity for 600s".into() });
        assert!(c.result.ends_with("Half way."));
        drop(tx);
    }

    /// A helper whose call waits on a clip that renders longer than the
    /// inactivity bound is not stalled: the clip lands and its report comes
    /// back whole. Once the wait ends, silence past the bound is a stall
    /// again. Audit 2026-10-07: video waits run up to 600 s, the bound is
    /// 600 s, and shot helpers were ended just as their clips landed.
    #[tokio::test(start_paused = true)]
    async fn a_call_waiting_on_a_render_is_not_silence() {
        let dir = tempfile::tempdir().unwrap();
        let waiting = std::sync::Arc::new(tools::Waiting::default());
        let (tx, rx) = mpsc::channel(4);
        tx.send(tool_call()).await.unwrap();
        let rendering = waiting.enter();
        let helper = tokio::spawn(async move {
            // Twice the bound with no event at all, waiting on the render.
            tokio::time::sleep(Duration::from_secs(1_200)).await;
            drop(rendering);
            tx.send(StreamEvent::text("Shot 2 is at clips/s2.mp4.")).await.unwrap();
            tx.send(StreamEvent::done_with_reason("end_turn")).await.unwrap();
        });
        let cancel = CancellationToken::new();
        let got = collect(rx, &cancel, Duration::from_secs(600), &waiting, |_| {}).await;
        helper.await.unwrap();
        assert!(!cancel.is_cancelled(), "a helper waiting on a render is never ended");
        let c = got.into_completion("h-1", "shot 2", dir.path(), REPORT_LIMIT);
        assert_eq!(c.status, CompletionStatus::Done);
        assert_eq!(c.result, "Shot 2 is at clips/s2.mp4.");

        // The wait over, silence past the bound ends the helper as before.
        let (tx, rx) = mpsc::channel::<StreamEvent>(4);
        let got = collect(rx, &cancel, Duration::from_secs(600), &waiting, |_| {}).await;
        assert!(cancel.is_cancelled(), "silence with nothing waited on is a stall");
        assert!(got.stalled.is_some());
        drop(tx);
    }

    #[test]
    fn clip_is_char_safe() {
        assert_eq!(clip_chars("ééé", 2), Some("éé"));
        assert_eq!(clip_chars("éé", 2), None);
    }
}
