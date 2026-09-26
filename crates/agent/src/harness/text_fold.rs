//! Whether a paragraph the model wrote between tool calls stays in the
//! reply or folds into the turn's work.
//!
//! Every text segment of a turn gets a verdict once something follows it: the
//! next tool call. The verdict goes out on the stream (`TextVerdict`) and is
//! stored on the segment's content block, so a reloaded thread renders the
//! same. The segment still streaming at the end of the turn has none: it is
//! the answer and stays in view. When the turn ends with every segment folded
//! and no answer after them, the longest folded segment is shown instead, so
//! a turn always leaves something to read.

use serde::{Deserialize, Serialize};

/// A segment's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fold {
    /// Prose in the reply.
    Shown,
    /// A note row inside the turn's work.
    Folded,
}

impl Fold {
    pub fn as_str(self) -> &'static str {
        match self {
            Fold::Shown => "shown",
            Fold::Folded => "folded",
        }
    }
}

/// A segment this many words long, or this many sentences, says something.
const SUBSTANTIAL_WORDS: usize = 30;
const SUBSTANTIAL_SENTENCES: usize = 3;
/// After this many tool calls a short segment is a step in the work.
const DEEP_IN_TURN: usize = 3;

/// Words a segment opens with when it only announces the next step.
const CONTINUATION_OPENERS: &[&str] = &[
    "now",
    "next",
    "let me",
    "let's",
    "i'll",
    "i will",
    "first",
    "then",
    "ok",
    "okay",
    "alright",
    "all right",
    "checking",
    "looking",
    "running",
    "trying",
    "good",
    "great",
    "perfect",
    "time to",
    "going to",
    "also",
    "so",
];

/// The verdict for a segment the next tool call has closed. `first_in_turn`:
/// no text came before it in the turn; `tools_before`: the tool calls the
/// turn made before it.
///
/// A question to the owner and a segment that says something (long, or
/// several sentences) stay. One that only announces the next step folds.
/// Otherwise the first text of the turn stays, and a short one deep in the
/// turn is a step in the work.
pub fn verdict(text: &str, first_in_turn: bool, tools_before: usize) -> Fold {
    let text = text.trim();
    if asks(text) || substantial(text) {
        return Fold::Shown;
    }
    if announces_next_step(text) {
        return Fold::Folded;
    }
    if !first_in_turn && tools_before >= DEEP_IN_TURN {
        return Fold::Folded;
    }
    Fold::Shown
}

/// Ends on a question to the reader.
fn asks(text: &str) -> bool {
    text.trim_end_matches(|c: char| {
        matches!(c, '*' | '_' | '`' | '"' | '\'' | ')' | '”' | '’') || c.is_whitespace()
    })
    .ends_with('?')
}

fn substantial(text: &str) -> bool {
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    let sentences = text
        .split(['.', '!', '?', '\n'])
        .filter(|s| {
            s.split_whitespace()
                .filter(|w| w.chars().any(char::is_alphanumeric))
                .count()
                >= 3
        })
        .count();
    words >= SUBSTANTIAL_WORDS || sentences >= SUBSTANTIAL_SENTENCES
}

/// Opens with a continuation word ("Let me…", "Now…", "Great,…").
fn announces_next_step(text: &str) -> bool {
    let lead: String = text
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .take(24)
        .collect::<String>()
        .to_lowercase()
        .replace('’', "'");
    CONTINUATION_OPENERS.iter().any(|opener| {
        lead.strip_prefix(opener)
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// One text segment of the turn.
#[derive(Debug, Default)]
struct Segment {
    /// Its length, in characters other than whitespace.
    chars: usize,
    text: String,
    fold: Option<Fold>,
    /// Where it is stored (the safety net rewrites its verdict there).
    row: Option<StoredRow>,
}

/// A segment's stored block: the reply row, and the index of its block in
/// the row's `metadata.contentBlocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRow {
    pub message_id: String,
    pub block: usize,
}

/// The turn's text segments, in the order they streamed. A segment is the
/// text between two tool calls: text streaming on after text (a retried or
/// resumed call) continues the open one, as the clients render it.
#[derive(Debug, Default)]
pub struct TurnFolds {
    segments: Vec<Segment>,
    open: bool,
    tools: usize,
}

impl TurnFolds {
    /// Text streamed.
    pub fn text(&mut self, chunk: &str) {
        if !self.open {
            self.segments.push(Segment::default());
            self.open = true;
        }
        if let Some(seg) = self.segments.last_mut() {
            seg.chars += chunk.chars().filter(|c| !c.is_whitespace()).count();
            seg.text.push_str(chunk);
        }
    }

    /// A tool call streamed: the open segment, if any, gets its verdict,
    /// returned with the segment's index. Whitespace alone is no segment.
    pub fn tool_call(&mut self) -> Option<(usize, Fold)> {
        if self.open && self.segments.last().is_some_and(|s| s.chars == 0) {
            self.segments.pop();
            self.open = false;
        }
        let closed = if self.open {
            self.open = false;
            let index = self.segments.len() - 1;
            let first = index == 0;
            let seg = &mut self.segments[index];
            let fold = verdict(&seg.text, first, self.tools);
            seg.fold = Some(fold);
            seg.text.clear();
            Some((index, fold))
        } else {
            None
        };
        self.tools += 1;
        closed
    }

    /// The verdict of the segment now closed, if the reply just stored
    /// holds it: the last decided segment with no row yet.
    pub fn stored(&mut self, row: StoredRow) {
        if let Some(seg) = self
            .segments
            .iter_mut()
            .rev()
            .find(|s| s.fold.is_some() && s.row.is_none())
        {
            seg.row = Some(row);
        }
    }

    /// At the end of the turn: when every segment folded and nothing
    /// followed them, the longest is shown instead. Returns its index and the
    /// row to rewrite.
    pub fn safety_net(&mut self) -> Option<(usize, Option<StoredRow>)> {
        let visible = self
            .segments
            .iter()
            .any(|s| s.fold != Some(Fold::Folded) && s.chars > 0);
        if visible {
            return None;
        }
        let (index, seg) = self
            .segments
            .iter_mut()
            .enumerate()
            .filter(|(_, s)| s.chars > 0)
            .max_by_key(|(i, s)| (s.chars, std::cmp::Reverse(*i)))?;
        seg.fold = Some(Fold::Shown);
        Some((index, seg.row.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNERS_EXAMPLE: &str = "That worked — the workflow was created with 2 activities and proper steps. \
        The problem is that update_employee with automations stored them as metadata… I need to recreate all \
        10 workflows properly through create_workflow. Let me delete the bad ones first, then rebuild";

    #[test]
    fn a_result_report_deep_in_the_turn_stays_shown() {
        assert_eq!(verdict(OWNERS_EXAMPLE, false, 12), Fold::Shown);
    }

    #[test]
    fn a_short_next_step_after_many_calls_folds() {
        assert_eq!(
            verdict("Let me check the workflows.", false, 5),
            Fold::Folded
        );
    }

    #[test]
    fn a_question_stays_shown() {
        assert_eq!(
            verdict("Which calendar should I use?", false, 7),
            Fold::Shown
        );
        assert_eq!(
            verdict("Want me to send it to **Dana**?", false, 7),
            Fold::Shown
        );
    }

    #[test]
    fn the_first_text_of_the_turn_stays_shown() {
        // After four calls, the same short report stays as the turn's first
        // words and folds as a later step.
        assert_eq!(verdict("The export has 212 rows.", true, 4), Fold::Shown);
        assert_eq!(verdict("The export has 212 rows.", false, 4), Fold::Folded);
    }

    #[test]
    fn an_opener_folds_early_and_a_short_report_early_stays() {
        assert_eq!(
            verdict("Now I'll open the invoice.", false, 1),
            Fold::Folded
        );
        assert_eq!(verdict("Great, that worked.", false, 1), Fold::Folded);
        assert_eq!(verdict("The file has two sheets.", false, 1), Fold::Shown);
        assert_eq!(verdict("The file has two sheets.", false, 4), Fold::Folded);
        // "so" is a word, not a prefix: "Some" is not an opener.
        assert_eq!(verdict("Some rows are empty.", false, 1), Fold::Shown);
    }

    #[test]
    fn a_tool_call_closes_the_open_segment_once() {
        let mut t = TurnFolds::default();
        t.text("Your calendar has two conflicts.");
        assert_eq!(t.tool_call(), Some((0, Fold::Shown)));
        // A second call in the same round closes nothing.
        assert_eq!(t.tool_call(), None);
        t.text("Let me ");
        t.text("check the workflows.");
        assert_eq!(t.tool_call(), Some((1, Fold::Folded)));
    }

    #[test]
    fn the_safety_net_shows_the_longest_when_all_folded() {
        let mut t = TurnFolds {
            tools: 4,
            ..TurnFolds::default()
        };
        // Two folded segments, then the turn ends on a tool call.
        for text in [
            "Now the next file.",
            "Checking the rest of the invoices now.",
        ] {
            t.text(text);
            assert_eq!(t.tool_call().map(|v| v.1), Some(Fold::Folded));
        }
        assert_eq!(t.safety_net().map(|v| v.0), Some(1));
        // Once shown, the net has nothing left to do.
        assert_eq!(t.safety_net(), None);
    }

    #[test]
    fn the_safety_net_leaves_a_turn_with_an_answer_alone() {
        let mut t = TurnFolds {
            tools: 4,
            ..TurnFolds::default()
        };
        t.text("Now the next file.");
        t.tool_call();
        t.text("Done: all ten are rebuilt.");
        assert_eq!(t.safety_net(), None);
    }
}
