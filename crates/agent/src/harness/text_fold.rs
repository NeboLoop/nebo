//! Whether a paragraph the model wrote between tool calls stays in the
//! reply or folds into the turn's work.
//!
//! Every text segment of a turn gets a verdict once something follows it: the
//! next tool call. The verdict goes out on the stream (`TextVerdict`) and is
//! stored on the segment's content block, so a reloaded thread renders the
//! same. The segment still streaming at the end of the turn has none: it is
//! the answer.
//!
//! Text between calls is the employee telling the owner what it found and
//! what it is doing, in whatever language it writes: it is always shown. The
//! one thing that folds is a segment that says word for word what an earlier
//! one in the turn said (`normalized`), and an earlier segment the answer
//! repeats word for word. No opener words, no length or sentence rules, no
//! count of calls: those read English only and folded findings.

use serde::{Deserialize, Serialize};

/// A segment's verdict. The stored format stays `shown` / `folded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fold {
    /// Text the owner reads.
    Shown,
    /// A word-for-word repeat of text the turn already shows.
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

/// A segment's text as compared for a repeat: its runs of letters and
/// digits (any script: `char::is_alphanumeric` is Unicode-aware), lowercased
/// and joined by one space. Whitespace, punctuation (`.`, `。`, `！`, `—`)
/// and markdown marks make no difference; a word does.
fn normalized(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// One text segment of the turn.
#[derive(Debug, Default)]
struct Segment {
    /// Its length, in characters other than whitespace.
    chars: usize,
    text: String,
    /// Its normalized text, once it is closed.
    said: String,
    fold: Option<Fold>,
    /// Where it is stored (the end of the turn rewrites its verdict there).
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
    /// It folds only when an earlier segment said exactly the same.
    pub fn tool_call(&mut self) -> Option<(usize, Fold)> {
        if self.open && self.segments.last().is_some_and(|s| s.chars == 0) {
            self.segments.pop();
            self.open = false;
        }
        if !self.open {
            return None;
        }
        self.open = false;
        let index = self.segments.len() - 1;
        let said = normalized(&self.segments[index].text);
        let again = !said.is_empty() && self.segments[..index].iter().any(|s| s.said == said);
        let fold = if again { Fold::Folded } else { Fold::Shown };
        let seg = &mut self.segments[index];
        seg.fold = Some(fold);
        seg.said = said;
        seg.text.clear();
        Some((index, fold))
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

    /// At the end of the turn: the earlier shown segments the answer (the
    /// segment still open) says again word for word fold, so the turn shows
    /// it once. Returns each one's index and the row to rewrite.
    pub fn fold_repeats_of_the_answer(&mut self) -> Vec<(usize, Option<StoredRow>)> {
        let Some(answer) = self.segments.last().filter(|s| self.open && s.fold.is_none()) else {
            return Vec::new();
        };
        let said = normalized(&answer.text);
        if said.is_empty() {
            return Vec::new();
        }
        let last = self.segments.len() - 1;
        self.segments[..last]
            .iter_mut()
            .enumerate()
            .filter(|(_, s)| s.fold == Some(Fold::Shown) && s.said == said)
            .map(|(i, s)| {
                s.fold = Some(Fold::Folded);
                (i, s.row.clone())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Close one segment of `text` with a call.
    fn close(t: &mut TurnFolds, text: &str) -> Option<(usize, Fold)> {
        t.text(text);
        t.tool_call()
    }

    const OWNERS_EXAMPLE: &str = "That worked — the workflow was created with 2 activities and proper steps. \
        The problem is that update_employee with automations stored them as metadata… I need to recreate all \
        10 workflows properly through create_workflow. Let me delete the bad ones first, then rebuild";

    /// Text between calls is shown however short, whatever it opens with
    /// and however deep in the turn: findings never fold.
    #[test]
    fn text_between_calls_is_always_shown() {
        let mut t = TurnFolds::default();
        for _ in 0..5 {
            t.tool_call();
        }
        assert_eq!(close(&mut t, "The export has 212 rows."), Some((0, Fold::Shown)));
        assert_eq!(close(&mut t, "So the March invoices are missing."), Some((1, Fold::Shown)));
        assert_eq!(close(&mut t, "Also: two are duplicates."), Some((2, Fold::Shown)));
        assert_eq!(close(&mut t, "Let me check the workflows."), Some((3, Fold::Shown)));
        assert_eq!(close(&mut t, OWNERS_EXAMPLE), Some((4, Fold::Shown)));
    }

    /// Japanese, Chinese, Spanish and German are read the same way: no
    /// sentence or word rules that assume spaces and full stops.
    #[test]
    fn every_language_is_shown() {
        let mut t = TurnFolds::default();
        for _ in 0..4 {
            t.tool_call();
        }
        for (i, text) in [
            "請求書を確認しました。3月分が2件足りません！",
            "导出文件有212行。",
            "Ahora reviso las facturas de marzo.",
            "Also gut, die Datei hat zwei Blätter.",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(close(&mut t, text), Some((i, Fold::Shown)), "{text}");
        }
    }

    /// Only a word-for-word repeat folds: whitespace, punctuation and case
    /// make no difference, in any script; one word more or less does.
    #[test]
    fn only_an_exact_repeat_folds() {
        let mut t = TurnFolds::default();
        assert_eq!(close(&mut t, "Four clips in, still holding."), Some((0, Fold::Shown)));
        assert_eq!(close(&mut t, "four clips in — still   holding!"), Some((1, Fold::Folded)));
        assert_eq!(close(&mut t, "Got it, four clips in, still holding."), Some((2, Fold::Shown)));
        assert_eq!(close(&mut t, "請求書を確認しました。"), Some((3, Fold::Shown)));
        assert_eq!(close(&mut t, "請求書を確認しました！"), Some((4, Fold::Folded)));
        assert_eq!(close(&mut t, "請求書を確認しています。"), Some((5, Fold::Shown)));
        assert_eq!(close(&mut t, "Überprüfe die Rechnungen."), Some((6, Fold::Shown)));
        assert_eq!(close(&mut t, "ÜBERPRÜFE die Rechnungen"), Some((7, Fold::Folded)));
    }

    #[test]
    fn normalizing_keeps_words_and_drops_marks() {
        assert_eq!(normalized("**Done.**  Next: `rows`"), "done next rows");
        assert_eq!(normalized("完了。次へ！"), "完了 次へ");
        assert_eq!(normalized("¿Listo?"), "listo");
        assert_eq!(normalized("..."), "");
    }

    #[test]
    fn a_tool_call_closes_the_open_segment_once() {
        let mut t = TurnFolds::default();
        assert_eq!(close(&mut t, "Your calendar has two conflicts."), Some((0, Fold::Shown)));
        // A second call in the same round closes nothing.
        assert_eq!(t.tool_call(), None);
        t.text("Let me ");
        t.text("check the workflows.");
        assert_eq!(t.tool_call(), Some((1, Fold::Shown)));
        // Whitespace alone is no segment.
        t.text("  \n");
        assert_eq!(t.tool_call(), None);
    }

    /// An answer that repeats an earlier segment word for word folds that
    /// one, where it is stored too; the answer stays.
    #[test]
    fn an_answer_that_repeats_an_earlier_segment_folds_that_one() {
        let mut t = TurnFolds::default();
        close(&mut t, "Five clips in, still holding.");
        let row = StoredRow { message_id: "m1".into(), block: 0 };
        t.stored(row.clone());
        t.text("Five clips in — still holding!");
        assert_eq!(t.fold_repeats_of_the_answer(), vec![(0, Some(row))]);
        assert_eq!(t.fold_repeats_of_the_answer(), vec![]);
    }

    #[test]
    fn an_answer_that_says_more_folds_nothing() {
        let mut t = TurnFolds::default();
        close(&mut t, "Five clips in, still holding.");
        t.text("Five clips in, still holding. Here is the plan.");
        assert_eq!(t.fold_repeats_of_the_answer(), vec![]);
        // A turn that ended on a call has no answer to compare.
        let mut t = TurnFolds::default();
        close(&mut t, "Five clips in, still holding.");
        assert_eq!(t.fold_repeats_of_the_answer(), vec![]);
    }

    /// "212 rows" is a finding: a later short answer never folds it.
    #[test]
    fn a_short_answer_never_folds_the_report_before_it() {
        let mut t = TurnFolds::default();
        close(&mut t, "The export has 212 rows.");
        t.text("Done, the export is ready.");
        assert_eq!(t.fold_repeats_of_the_answer(), vec![]);
    }
}
