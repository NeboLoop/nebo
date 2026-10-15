//! The owner's words as a search of the full-text indexes (`chat_messages_fts`,
//! `memories_fts`, `memory_chunks_fts`), the one way every search builds its
//! query.
//!
//! The indexes use FTS5's `trigram` tokenizer (migration 0243): it indexes
//! every three characters, so a word is found anywhere inside a sentence,
//! also in Japanese, Chinese, Korean and Thai, which write no spaces between
//! words and which the default tokenizer kept as one token per run of text.
//! A trigram index can't look up a word shorter than three characters
//! (会議, 회의, "AI"): those words are matched with `LIKE` on the same rows.
//! A short word of a script written without spaces between words, or with
//! particles attached to them (会議, 회의가's 회의), is a real word anywhere
//! in the text; one of a spaced script ("a", "of", "AI") is mostly a piece
//! of other words, so an any-word search leaves it out unless the query has
//! nothing else.

use rusqlite::types::Value;

/// Shorter words than this are matched with `LIKE`, not the index.
const TRIGRAM: usize = 3;

/// The owner's words, split by what can look them up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Terms {
    /// Three characters or more: looked up in the index.
    indexed: Vec<String>,
    /// One or two characters of a script written without spaces
    /// ([`unspaced`]): matched with `LIKE`.
    short: Vec<String>,
    /// One or two characters of a spaced script: matched with `LIKE` by an
    /// every-word search, and by an any-word one only when nothing else is.
    spaced_short: Vec<String>,
}

/// A character of a script that writes no spaces between words (Han, kana,
/// Thai, Lao, Khmer, Myanmar), or attaches particles to them (Hangul).
fn unspaced(c: char) -> bool {
    matches!(c as u32,
        0x0E00..=0x0EFF // Thai, Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1100..=0x11FF // Hangul Jamo
        | 0x1780..=0x17FF // Khmer
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3130..=0x318F // Hangul compatibility Jamo
        | 0x31F0..=0x31FF // Katakana extensions
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0x20000..=0x2FFFF) // CJK extensions B and later
}

impl Terms {
    /// The whitespace-separated words of `query`, each without the
    /// punctuation around it ("rates?", "会議。") and without double quotes
    /// (which would end an FTS5 phrase). A short word is left with letters
    /// and digits (and `+`, `#`) only, so it is literal in a `LIKE` pattern.
    pub(crate) fn parse(query: &str) -> Self {
        let mut terms = Self::default();
        for word in query.split_whitespace() {
            let word: String = word
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
                .chars()
                .filter(|c| *c != '"')
                .collect();
            if word.is_empty() {
                continue;
            }
            if word.chars().count() >= TRIGRAM {
                terms.indexed.push(word);
            } else if word.chars().any(unspaced) {
                terms.short.push(word);
            } else {
                terms.spaced_short.push(word);
            }
        }
        terms
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.indexed.is_empty() && self.short.is_empty() && self.spaced_short.is_empty()
    }

    /// The FTS5 `MATCH` expression of the indexed words, each a quoted
    /// phrase (a substring, to the trigram index), joined by `op` (`" "`:
    /// every word; `" OR "`: any). None when every word is short.
    pub(crate) fn match_expr(&self, op: &str) -> Option<String> {
        (!self.indexed.is_empty()).then(|| self.indexed.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(op))
    }

    /// The conditions an every-word search puts on its rows: `table`'s
    /// `MATCH` of the indexed words, and `column LIKE` for each short word.
    /// Their values are appended to `values`, numbered after those already
    /// there.
    pub(crate) fn every_word(&self, table: &str, column: &str, values: &mut Vec<Value>) -> Vec<String> {
        let mut conds = Vec::new();
        if let Some(expr) = self.match_expr(" ") {
            values.push(Value::Text(expr));
            conds.push(format!("{table} MATCH ?{}", values.len()));
        }
        for word in self.short.iter().chain(&self.spaced_short) {
            values.push(Value::Text(format!("%{word}%")));
            conds.push(format!("{column} LIKE ?{}", values.len()));
        }
        conds
    }

    /// An any-word search's `LIKE` patterns (`%word%`): its short words of
    /// unspaced scripts, or, when the query has nothing else, its short
    /// spaced ones ("AI").
    pub(crate) fn any_word_patterns(&self) -> Vec<String> {
        let words = if self.indexed.is_empty() && self.short.is_empty() { &self.spaced_short } else { &self.short };
        words.iter().map(|w| format!("%{w}%")).collect()
    }
}

/// An OR search's hits from the index (BM25 ranks, negative, lower is
/// better) and from its short words (minus the number of short words a row
/// holds), one list: a row found both ways adds the two, best first, at most
/// `limit`.
pub(crate) fn merge_ranked(indexed: Vec<(i64, f64)>, short: Vec<(i64, f64)>, limit: i64) -> Vec<(i64, f64)> {
    let mut merged: Vec<(i64, f64)> = indexed;
    for (id, rank) in short {
        match merged.iter_mut().find(|(m, _)| *m == id) {
            Some(hit) => hit.1 += rank,
            None => merged.push((id, rank)),
        }
    }
    merged.sort_by(|a, b| a.1.total_cmp(&b.1));
    merged.truncate(usize::try_from(limit).unwrap_or(0));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_into_indexed_and_short() {
        let t = Terms::parse("会議 ประชุม 회의가 rates? \"AI\" 会議。 c++ to ...");
        assert_eq!(t.indexed, ["ประชุม", "회의가", "rates", "c++"]);
        assert_eq!(t.short, ["会議", "会議"]);
        assert_eq!(t.spaced_short, ["AI", "to"]);
        assert_eq!(t.match_expr(" OR ").as_deref(), Some("\"ประชุม\" OR \"회의가\" OR \"rates\" OR \"c++\""));
        assert!(Terms::parse("  ... \"\" 5% ").spaced_short == ["5"]);
        assert!(Terms::parse("  ... \"\" ").is_empty());
        assert_eq!(Terms::parse("会議").match_expr(" "), None);
    }

    #[test]
    fn every_word_numbers_its_values_after_the_others() {
        let mut values = vec![Value::Null];
        let conds = Terms::parse("rates 会議 to").every_word("f", "m.content", &mut values);
        assert_eq!(conds, ["f MATCH ?2", "m.content LIKE ?3", "m.content LIKE ?4"]);
        assert_eq!(values[1], Value::Text("\"rates\"".into()));
        assert_eq!(values[2], Value::Text("%会議%".into()));
        assert_eq!(values[3], Value::Text("%to%".into()));
    }

    /// "a" and "of" in a prompt would match nearly every memory: an any-word
    /// search uses a short spaced word only when it is all there is.
    #[test]
    fn an_any_word_search_skips_short_spaced_words_unless_alone() {
        assert_eq!(Terms::parse("a summary of 会議").any_word_patterns(), ["%会議%"]);
        assert!(Terms::parse("a summary of it").any_word_patterns().is_empty());
        assert_eq!(Terms::parse("AI").any_word_patterns(), ["%AI%"]);
    }

    #[test]
    fn hits_found_both_ways_add_up() {
        let merged = merge_ranked(vec![(1, -2.0), (2, -0.5)], vec![(2, -2.0), (3, -1.0)], 2);
        assert_eq!(merged, [(2, -2.5), (1, -2.0)]);
    }
}
