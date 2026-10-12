//! A recording too long or too large for one transcription request, in
//! parts: the Nebo Media plugin pulls its sound out as small mono speech
//! and cuts it in pauses (`audio convert --preset speech --split`), each
//! part is transcribed on its own, and the parts are stitched back into
//! one transcript on the recording's clock.
//!
//! A transcriber labels the voices of each request on their own, so "S1"
//! in one part need not be "S1" in the next. Each part after the first
//! starts a little before its cut (the overlap) and is heard again there:
//! the words both parts heard say which of this part's voices is which of
//! the last part's. A voice the overlap cannot place gets a label of its
//! own, and the result says so.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde_json::Value;

use super::audio::{Piece, Transcript, one_voice_unlabelled};

/// One part as the plugin wrote it: its file, where its sound starts and
/// ends in the recording, and where its own words begin (before `keep`
/// they are the part before's, heard again).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub file: PathBuf,
    pub start: f64,
    pub keep: f64,
    pub end: f64,
}

/// The plugin's `Parts:` line: the recording's length and its parts.
pub fn parse(stdout: &str) -> Result<(f64, Vec<Part>), String> {
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("Parts: "))
        .ok_or_else(|| "Nebo Media did not list the parts it wrote.".to_string())?;
    let v: Value = serde_json::from_str(line).map_err(|e| format!("Nebo Media's list of parts could not be read: {e}"))?;
    let num = |p: &Value, k: &str| p.get(k).and_then(Value::as_f64).ok_or_else(|| format!("a part has no `{k}`"));
    let parts = v
        .get("parts")
        .and_then(Value::as_array)
        .ok_or("Nebo Media's list has no parts.")?
        .iter()
        .map(|p| {
            Ok(Part {
                file: PathBuf::from(p.get("file").and_then(Value::as_str).ok_or("a part has no `file`")?),
                start: num(p, "start")?,
                keep: num(p, "keep")?,
                end: num(p, "end")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if parts.is_empty() {
        return Err("Nebo Media wrote no parts.".to_string());
    }
    let duration = v.get("duration").and_then(Value::as_f64).unwrap_or(parts[parts.len() - 1].end);
    Ok((duration, parts))
}

/// One transcript from the parts' (each on its own part's clock, with its
/// own voices' labels): every time moved onto the recording's clock, the
/// overlap's words kept only from the part before, and each voice labelled
/// across parts. With how many voices after the first part were matched
/// to one before, and how many could not be.
pub struct Stitched {
    pub transcript: Transcript,
    pub matched: usize,
    pub unmatched: usize,
}

/// Seconds apart two hearings of one word in an overlap may be.
const SAME_WORD: f64 = 0.5;
/// Words a voice must share with one voice of the part before to be it.
const MIN_VOTES: usize = 2;

pub fn stitch(parts: &[Part], each: Vec<Transcript>, duration: f64) -> Stitched {
    let mut out = Transcript { language: String::new(), duration, words: vec![], segments: vec![] };
    let mut next = 1usize;
    let (mut matched, mut unmatched) = (0, 0);
    // The last part's words on the recording's clock, with their labels as
    // stitched: what the next part's overlap is matched against.
    let mut before: Vec<Piece> = vec![];
    for (k, (part, t)) in parts.iter().zip(each).enumerate() {
        if out.language.is_empty() {
            out.language = t.language.clone();
        }
        let moved = |p: &Piece| Piece { start: ms(p.start + part.start), end: ms(p.end + part.start), ..p.clone() };
        let words: Vec<Piece> = t.words.iter().map(moved).collect();
        let segments: Vec<Piece> = t.segments.iter().map(moved).collect();
        let own = |p: &&Piece| k == 0 || p.start >= part.keep;

        // Votes: this part's voice → the voice the part before heard
        // saying the same word at the same moment.
        let mut votes: HashMap<&str, BTreeMap<&str, usize>> = HashMap::new();
        for w in words.iter().filter(|w| w.start < part.keep) {
            let (Some(local), Some(said)) = (w.speaker.as_deref(), bare(&w.text)) else { continue };
            if let Some(b) = before.iter().find(|b| (b.start - w.start).abs() <= SAME_WORD && bare(&b.text).as_deref() == Some(said.as_str()))
                && let Some(global) = b.speaker.as_deref()
            {
                *votes.entry(local).or_default().entry(global).or_default() += 1;
            }
        }
        // This part's voices in order of first appearance in its own words.
        let mut voices: Vec<&str> = vec![];
        for p in words.iter().chain(&segments).filter(own) {
            if let Some(s) = p.speaker.as_deref()
                && !voices.contains(&s)
            {
                voices.push(s);
            }
        }
        // Strongest agreements first; each voice before is taken once.
        let mut pairs: Vec<(&str, &str, usize)> =
            votes.iter().flat_map(|(l, g)| g.iter().map(move |(g, n)| (*l, *g, *n))).filter(|x| x.2 >= MIN_VOTES).collect();
        pairs.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)).then(a.1.cmp(b.1)));
        let mut label: HashMap<&str, String> = HashMap::new();
        for (l, g, _) in pairs {
            if voices.contains(&l) && !label.contains_key(l) && !label.values().any(|v| v.as_str() == g) {
                label.insert(l, g.to_string());
            }
        }
        for v in &voices {
            if label.contains_key(v) {
                matched += 1;
            } else {
                label.insert(v, format!("S{next}"));
                next += 1;
                if k > 0 {
                    unmatched += 1;
                }
            }
        }
        let relabel = |p: &Piece| Piece { speaker: p.speaker.as_deref().and_then(|s| label.get(s).cloned()), ..p.clone() };
        before = words.iter().map(relabel).collect();
        out.words.extend(words.iter().filter(own).map(relabel));
        out.segments.extend(segments.iter().filter(own).map(relabel));
    }
    one_voice_unlabelled(&mut out);
    out.words.sort_by(|a, b| a.start.total_cmp(&b.start));
    out.segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    Stitched { transcript: out, matched, unmatched }
}

/// A word as compared across parts: lowercase letters and digits only.
fn bare(text: &str) -> Option<String> {
    let b: String = text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    (!b.is_empty()).then_some(b)
}

/// Seconds to the millisecond.
fn ms(s: f64) -> f64 {
    (s.max(0.0) * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, speaker: &str) -> Piece {
        Piece { text: text.into(), start, end: start + 0.3, speaker: Some(speaker.into()) }
    }

    fn part(start: f64, keep: f64, end: f64) -> Part {
        Part { file: PathBuf::from("p.m4a"), start, keep, end }
    }

    fn t(words: Vec<Piece>) -> Transcript {
        Transcript { language: "en".into(), duration: 0.0, words, segments: vec![] }
    }

    #[test]
    fn the_parts_line_is_read() {
        let out = "Checked: opens\nResult: /x/a-1.m4a (…)\nParts: {\"duration\": 2500.5, \"parts\": [{\"file\": \"/x/a-1.m4a\", \"start\": 0, \"keep\": 0, \"end\": 1190.2}, {\"file\": \"/x/a-2.m4a\", \"start\": 1160.2, \"keep\": 1190.2, \"end\": 2500.5}]}\n";
        let (d, p) = parse(out).unwrap();
        assert_eq!(d, 2500.5);
        assert_eq!(p[1], Part { file: "/x/a-2.m4a".into(), start: 1160.2, keep: 1190.2, end: 2500.5 });
        assert!(parse("Result: x").unwrap_err().contains("did not list"));
    }

    /// Times move onto the recording's clock; the overlap's words come
    /// only from the part before; the voices the overlap hears in both
    /// keep their labels, though the second part named them the other way
    /// round; a voice the overlap never heard gets a label of its own.
    #[test]
    fn parts_stitch_onto_one_clock_with_their_voices_matched() {
        let parts = [part(0.0, 0.0, 100.0), part(90.0, 100.0, 200.0)];
        let first = t(vec![w("Hello", 1.0, "S1"), w("there.", 1.4, "S1"), w("Fine,", 92.0, "S2"), w("thanks.", 92.5, "S2"), w("Good.", 96.0, "S1"), w("Great.", 97.0, "S1")]);
        // The second part's clock starts at 90 s; its S1 is the first part's S2.
        let second = t(vec![
            w("Fine,", 2.05, "S1"),
            w("thanks.", 2.5, "S1"),
            w("Good.", 6.1, "S2"),
            w("Great.", 7.0, "S2"),
            w("Next", 12.0, "S1"),
            w("question?", 12.4, "S2"),
            w("Hi!", 50.0, "S3"),
        ]);
        let s = stitch(&parts, vec![first, second], 200.0);
        let got: Vec<(&str, f64, Option<&str>)> = s.transcript.words.iter().map(|p| (p.text.as_str(), p.start, p.speaker.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("Hello", 1.0, Some("S1")),
                ("there.", 1.4, Some("S1")),
                ("Fine,", 92.0, Some("S2")),
                ("thanks.", 92.5, Some("S2")),
                ("Good.", 96.0, Some("S1")),
                ("Great.", 97.0, Some("S1")),
                ("Next", 102.0, Some("S2")),
                ("question?", 102.4, Some("S1")),
                ("Hi!", 140.0, Some("S3")),
            ]
        );
        assert!(s.transcript.words.windows(2).all(|p| p[0].start <= p[1].start), "monotonic");
        assert_eq!((s.matched, s.unmatched), (2, 1), "S1 and S2 in both parts; the second part's S3 is new");
        assert_eq!(s.transcript.duration, 200.0);
    }

    /// Without an overlap (or with too little heard twice), no voice is
    /// matched: the second part's voices are new labels, never presented
    /// as the first part's.
    #[test]
    fn voices_the_overlap_cannot_place_get_their_own_labels() {
        let parts = [part(0.0, 0.0, 10.0), part(10.0, 10.0, 20.0)];
        let s = stitch(&parts, vec![t(vec![w("a", 1.0, "S1"), w("b", 2.0, "S2")]), t(vec![w("c", 1.0, "S1"), w("d", 2.0, "S2")])], 20.0);
        let labels: Vec<_> = s.transcript.words.iter().map(|p| p.speaker.clone().unwrap()).collect();
        assert_eq!(labels, ["S1", "S2", "S3", "S4"]);
        assert_eq!((s.matched, s.unmatched), (0, 2));
    }

    /// One voice in the whole recording: no labels, as for one request.
    #[test]
    fn one_voice_across_parts_has_no_labels() {
        let parts = [part(0.0, 0.0, 10.0), part(5.0, 10.0, 20.0)];
        let s = stitch(&parts, vec![t(vec![w("one", 6.0, "S1"), w("two", 7.0, "S1")]), t(vec![w("one", 1.0, "S1"), w("two", 2.0, "S1"), w("three", 6.0, "S1")])], 20.0);
        assert_eq!(s.transcript.words.len(), 3);
        assert!(s.transcript.words.iter().all(|p| p.speaker.is_none()));
        assert_eq!(s.transcript.words[2].start, 11.0);
    }
}
