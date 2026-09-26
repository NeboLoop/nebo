//! Collecting a helper's result: its final message only (or its last words,
//! marked, when it stopped without one). A report past the cap goes through
//! the one spill path: the full text saved, a preview of its start inline.

use std::path::Path;
use std::time::Duration;

use ai::{StreamEvent, StreamEventType};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Completion, CompletionStatus};

/// A result longer than this is saved and previewed (the one spill path).
pub const RESULT_CAP: usize = 100_000;

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
}

impl Collected {
    pub fn failed(error: String) -> Self {
        Self { error: Some(error), ..Self::default() }
    }

    /// The completion this turn reports. A result over [`RESULT_CAP`] keeps
    /// its tail, and the full text is saved in `spill_dir`.
    pub fn into_completion(self, task_id: &str, description: &str, spill_dir: &Path) -> Completion {
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
            result: spill_if_long(&text, spill_dir),
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

/// Read a helper turn's events to the end. `on_event` sees every event (the
/// owner's progress line is built from it); nothing is forwarded to the
/// parent. The turn ends at `Done`, at a closed stream, when `cancel`
/// fires, or after `inactivity` without an event.
pub async fn collect(
    mut events: mpsc::Receiver<StreamEvent>,
    cancel: &CancellationToken,
    inactivity: Duration,
    mut on_event: impl FnMut(&StreamEvent),
) -> Collected {
    let mut out = Collected::default();
    let mut current = String::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                out.cancelled = true;
                break;
            }
            next = tokio::time::timeout(inactivity, events.recv()) => {
                let Ok(event) = next else {
                    cancel.cancel();
                    out.stalled = Some(inactivity);
                    break;
                };
                let Some(event) = event else { break };
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

/// `text`, or past [`RESULT_CAP`] characters the one spill path's note: the
/// full text saved in `spill_dir`, a preview of its start inline.
pub fn spill_if_long(text: &str, spill_dir: &Path) -> String {
    if text.chars().count() <= RESULT_CAP {
        return text.to_string();
    }
    tools::result_shape::persist(spill_dir, text)
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
        collect(rx, &CancellationToken::new(), Duration::from_secs(5), |_| {}).await
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
        let c = got.into_completion("h-1", "find it", dir.path());
        assert_eq!(c.status, CompletionStatus::Done);
        assert_eq!(c.result, "The invoice is missing from March.", "narration never returns");

        // Oversized: the one spill path, as for any tool result: the whole
        // text saved, its start previewed.
        let long = format!("THE START{}THE END", "é".repeat(RESULT_CAP + 50));
        let got = run(vec![tool_call(), StreamEvent::text(long.clone()), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "find it", dir.path());
        assert_eq!(c.status, CompletionStatus::Done);
        assert!(c.result.starts_with(tools::result_shape::SAVED_OUTPUT), "{}", &c.result[..200]);
        assert!(c.result.contains("THE START"));
        assert!(!c.result.contains("THE END"));
        let path = c.result.split("Saved in full at: ").nth(1).unwrap().lines().next().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), long, "the full text is on disk");
        assert!(c.result.chars().count() < 3_000);
    }

    /// A turn that ended at its step or spending limit is partial, whatever
    /// it last said.
    #[tokio::test]
    async fn a_helper_at_its_limit_reports_partial() {
        let dir = tempfile::tempdir().unwrap();
        for (reason, why) in [(STOP_MAX_STEPS, "hit its step limit"), (STOP_SPEND_CAP, "hit its spending limit")] {
            let got = run(vec![tool_call(), StreamEvent::text("Got through March."), StreamEvent::done_with_reason(reason)]).await;
            let c = got.into_completion("h-1", "find it", dir.path());
            assert_eq!(c.status, CompletionStatus::Partial { why: why.into() });
            assert_eq!(c.result, "Got through March.");
        }
    }

    #[tokio::test]
    async fn no_final_report_returns_marked_last_words_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![StreamEvent::text("Checked March, now April."), tool_call(), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "find it", dir.path());
        assert!(matches!(c.status, CompletionStatus::Partial { .. }));
        assert_eq!(c.result, format!("{NO_FINAL_REPORT}\nChecked March, now April."));
    }

    #[tokio::test]
    async fn an_error_fails_and_a_cancel_stops() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![StreamEvent::text("x"), StreamEvent::error("provider down")]).await;
        let c = got.into_completion("h-1", "d", dir.path());
        assert_eq!(c.status, CompletionStatus::Failed { error: "provider down".into() });

        let (_tx, rx) = mpsc::channel::<StreamEvent>(1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let got = collect(rx, &cancel, Duration::from_secs(5), |_| {}).await;
        assert_eq!(got.into_completion("h-1", "d", dir.path()).status, CompletionStatus::Stopped);
    }

    /// A turn that saw its cancel ends with `Done` saying so. The collector
    /// can read that event before it wakes to the cancel itself (both are
    /// ready, one is picked): the helper was stopped either way, never
    /// completed with a partial report its parent is woken for.
    #[tokio::test]
    async fn a_turn_that_ended_on_its_cancel_is_stopped_whichever_arrives_first() {
        let dir = tempfile::tempdir().unwrap();
        let got = run(vec![tool_call(), StreamEvent::text("Got through March."), StreamEvent::done_with_reason(STOP_CANCELLED)]).await;
        let c = got.into_completion("h-1", "find it", dir.path());
        assert_eq!(c.status, CompletionStatus::Stopped);
        assert_eq!(c.result, "Got through March.", "its last words, not a report");
    }

    #[tokio::test(start_paused = true)]
    async fn silence_ends_the_turn_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(4);
        tx.send(StreamEvent::text("Half way.")).await.unwrap();
        let cancel = CancellationToken::new();
        let got = collect(rx, &cancel, Duration::from_secs(600), |_| {}).await;
        assert!(cancel.is_cancelled(), "a stalled helper is ended");
        let c = got.into_completion("h-1", "d", dir.path());
        assert_eq!(c.status, CompletionStatus::Partial { why: "no activity for 600s".into() });
        assert!(c.result.ends_with("Half way."));
        drop(tx);
    }

    #[test]
    fn clip_is_char_safe() {
        assert_eq!(clip_chars("ééé", 2), Some("éé"));
        assert_eq!(clip_chars("éé", 2), None);
    }
}
