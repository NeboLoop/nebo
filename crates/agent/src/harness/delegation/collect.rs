//! Collecting a helper's result: its final message only (or its last words,
//! marked, when it stopped without one), tail kept past the cap with the full
//! text spilled to a file.

use std::path::Path;
use std::time::Duration;

use ai::{StreamEvent, StreamEventType};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Completion, CompletionStatus};

/// A result longer than this keeps its tail; the full text is saved.
pub const RESULT_CAP: usize = 100_000;

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
    pub usage: ai::UsageInfo,
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
            result: keep_tail(&text, spill_dir),
            usage: self.usage,
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
                    StreamEventType::Done => break,
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

/// `text` within [`RESULT_CAP`] characters: an over-long report keeps its
/// end, where the conclusion is, and says where the whole text was saved.
pub fn keep_tail(text: &str, spill_dir: &Path) -> String {
    let total = text.chars().count();
    if total <= RESULT_CAP {
        return text.to_string();
    }
    let start = text.char_indices().nth(total - RESULT_CAP).map_or(0, |(i, _)| i);
    let tail = &text[start..];
    let saved = match tools::result_shape::save(spill_dir, text) {
        Ok(path) => format!("the full report is saved at {}", path.display()),
        Err(e) => format!("saving the full report failed ({e})"),
    };
    format!("[The report was {total} characters; this is its last {RESULT_CAP}, and {saved}.]\n…{tail}")
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

        // Oversized: the tail (the conclusion) stays, the whole is saved.
        let long = format!("{}THE END", "é".repeat(RESULT_CAP + 50));
        let got = run(vec![tool_call(), StreamEvent::text(long.clone()), StreamEvent::done()]).await;
        let c = got.into_completion("h-1", "find it", dir.path());
        assert!(c.result.ends_with("THE END"));
        assert!(c.result.starts_with(&format!("[The report was {} characters", long.chars().count())));
        let path = c.result.split("saved at ").nth(1).unwrap().split(".]").next().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), long, "the full text is on disk");
        assert!(c.result.chars().count() < RESULT_CAP + 300);
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
