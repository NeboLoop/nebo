//! What a turn's words carry for the model — who sent them, what untrusted
//! content they hold — is metadata, never part of the words a person reads
//! (owner report 2026-09-29: "[Contains content from: web]" at the top of
//! every teammate's post, "[Coworker message from …]" in chats).
//!
//! The model-facing marks are written here, once, where a prompt is built
//! from metadata: [`colleague_mark`], [`provenance_mark`]. Rows stored before
//! the labels became metadata still begin with them; [`split`] takes them
//! off for a person's view and hands back what they said, so old rows read
//! clean without rewriting history.

use crate::provenance::ProvenanceClass;

const COLLEAGUE: &str = "[Coworker message from ";
/// How a team post the model reads begins (`server::coworker::team_envelope`).
pub const TEAM_POST: &str = "[Team \"";
const PROVENANCE: &str = "[Contains content from: ";
const REPLY: &str = "[Reply from ";
/// Labels older rows carry that name nobody, taken off the words whole.
const PLAIN: [&str; 4] = [
    "[You were @mentioned in a conversation. Respond helpfully.]",
    "[App interaction]",
    "[Workspace interaction]",
    "[Recent activity in this channel]",
];

/// How the model reads that words are a colleague's.
pub fn colleague_mark(name: &str) -> String {
    format!("{COLLEAGUE}{name}]")
}

/// `words` from colleague `name` as the model reads them: marked, unless
/// they already say who sent them (a team post, or a row stored with its
/// label in the words).
pub fn from_colleague(name: &str, words: &str) -> String {
    if words.starts_with(TEAM_POST) || split(words).colleague.is_some() {
        return words.to_string();
    }
    format!("{}\n\n{words}", colleague_mark(name))
}

/// How the model reads that words hold untrusted content: the classes past
/// `Coworker` (every colleague's words carry that one), or `None`.
pub fn provenance_mark(classes: &[ProvenanceClass]) -> Option<String> {
    let external: Vec<ProvenanceClass> = classes.iter().filter(|c| **c != ProvenanceClass::Coworker).copied().collect();
    if external.is_empty() {
        return None;
    }
    Some(format!("{PROVENANCE}{}]", crate::provenance::label_classes(&external)))
}

/// A stored row's words with the leading labels taken off.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split<'a> {
    /// What was said.
    pub text: &'a str,
    /// The colleague a label named as the sender.
    pub colleague: Option<&'a str>,
    /// The classes a provenance label named.
    pub provenance: Vec<ProvenanceClass>,
}

/// Take the leading labels off `content` (any number, in any order).
pub fn split(content: &str) -> Split<'_> {
    let mut out = Split { text: content, ..Split::default() };
    loop {
        let rest = out.text;
        if let Some((inner, after)) = bracket(rest, COLLEAGUE) {
            out.colleague = Some(inner);
            out.text = after;
        } else if let Some((inner, after)) = bracket(rest, REPLY) {
            out.colleague = Some(inner.split(" in team \"").next().unwrap_or(inner));
            out.text = after;
        } else if let Some((inner, after)) = bracket(rest, PROVENANCE) {
            for label in inner.split(", ") {
                if let Some(class) = class_labeled(label) {
                    out.provenance.push(class);
                }
            }
            out.text = after;
        } else if let Some(label) = PLAIN.iter().find(|l| rest.starts_with(**l)) {
            out.text = rest[label.len()..].trim_start();
        } else {
            return out;
        }
    }
}

/// `[<open><inner>]` at the start of `s`, and what follows it.
fn bracket<'a>(s: &'a str, open: &str) -> Option<(&'a str, &'a str)> {
    let rest = s.strip_prefix(open)?;
    let end = rest.find(']')?;
    let inner = &rest[..end];
    if inner.contains('\n') {
        return None;
    }
    Some((inner, rest[end + 1..].trim_start()))
}

fn class_labeled(label: &str) -> Option<ProvenanceClass> {
    use ProvenanceClass::*;
    [Web, ExternalEmail, Channel, Document, Phone, Coworker].into_iter().find(|c| c.label() == label.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every label older rows carry comes off, and says what it said.
    #[test]
    fn old_labels_come_off() {
        let s = split("[Coworker message from Top Coder]\n\n[Contains content from: web, external email]\nThe draft is ready.");
        assert_eq!(s.text, "The draft is ready.");
        assert_eq!(s.colleague, Some("Top Coder"));
        assert_eq!(s.provenance, vec![ProvenanceClass::Web, ProvenanceClass::ExternalEmail]);

        let s = split("[Reply from Proof Clerk in team \"Floor\"]\nFiled.");
        assert_eq!((s.text, s.colleague), ("Filed.", Some("Proof Clerk")));
        assert_eq!(split("[Reply from Billy]\nDone.").colleague, Some("Billy"));
        assert_eq!(split("[App interaction] The user clicked Save.").text, "The user clicked Save.");
        assert_eq!(split("[You were @mentioned in a conversation. Respond helpfully.]\n\nHi").text, "Hi");
        assert_eq!(split("[Recent activity in this channel]\nAva: hi\n").text, "Ava: hi\n");
    }

    /// Words that only start with a bracket stay as they are.
    #[test]
    fn plain_words_stay() {
        for words in ["[1] first item", "[Draft] the memo", "Contains content from: web", ""] {
            let s = split(words);
            assert_eq!(s.text, words);
            assert_eq!(s.colleague, None);
            assert!(s.provenance.is_empty());
        }
    }

    /// A colleague's words are marked once for the model.
    #[test]
    fn a_colleagues_words_are_marked_once() {
        assert_eq!(from_colleague("Ava", "The invoice is sent."), "[Coworker message from Ava]\n\nThe invoice is sent.");
        let old = "[Coworker message from Ava]\n\nThe invoice is sent.";
        assert_eq!(from_colleague("Ava", old), old);
        let post = "[Team \"Ops\" — run it]\n[Post from Ava]\n\nDone.";
        assert_eq!(from_colleague("Ava", post), post);
    }

    /// The marks round-trip through `split`: what the model reads, a person's
    /// view takes off.
    #[test]
    fn marks_round_trip() {
        let mark = provenance_mark(&[ProvenanceClass::Coworker, ProvenanceClass::Web]).unwrap();
        assert_eq!(mark, "[Contains content from: web]");
        assert_eq!(provenance_mark(&[ProvenanceClass::Coworker]), None);
        let words = format!("{}\n\n{mark}\nhello", colleague_mark("Ava"));
        let s = split(&words);
        assert_eq!((s.text, s.colleague, s.provenance), ("hello", Some("Ava"), vec![ProvenanceClass::Web]));
    }
}
