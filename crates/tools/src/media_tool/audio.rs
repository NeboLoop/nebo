//! Generated audio and transcripts: the request bodies for music and sound
//! effects, the AI-generated tag every made audio file carries, and the
//! transcript JSON Nebo Media reads (`docs/publishers-guide/transcript-json.md`).

use serde::Serialize;
use serde_json::{Value, json};

/// What every made audio file says in its comment metadata, as the Nebo
/// Media plugin's `--ai-generated` writes it and its probe reads it.
pub const AI_TAG: &str = "AI-generated";

/// The longest music track and sound effect Janus makes, in seconds.
pub const MAX_MUSIC_SECONDS: u64 = 300;
pub const MAX_SOUND_SECONDS: u64 = 30;

/// The `/v1/audio/generations` body for a music or sound call. Music is
/// instrumental unless it was given lyrics or told otherwise.
pub fn generation_body(kind: &str, input: &Value, format: &str) -> Value {
    let str_of = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let mut body = json!({
        "kind": kind,
        "prompt": str_of("prompt").unwrap_or(""),
        "response_format": format,
    });
    if let Some(model) = str_of("model") {
        body["model"] = json!(model);
    }
    let max = if kind == "music" {
        MAX_MUSIC_SECONDS
    } else {
        MAX_SOUND_SECONDS
    };
    if let Some(s) = input.get("seconds").and_then(Value::as_u64) {
        body["seconds"] = json!(s.clamp(1, max));
    }
    if kind == "music" {
        let lyrics = str_of("lyrics");
        let instrumental = input
            .get("instrumental")
            .and_then(Value::as_bool)
            .unwrap_or(lyrics.is_none());
        body["instrumental"] = json!(instrumental);
        if let (Some(l), false) = (lyrics, instrumental) {
            body["lyrics"] = json!(l);
        }
    }
    body
}

/// The file extension for audio Janus answered: from the bytes when they
/// say, else from the content type, else `asked`.
pub fn audio_ext<'a>(bytes: &[u8], content_type: &str, asked: &'a str) -> &'a str {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "wav";
    }
    if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        return "mp3";
    }
    if bytes.starts_with(b"fLaC") {
        return "flac";
    }
    if bytes.starts_with(b"OggS") {
        return "ogg";
    }
    match content_type.split(';').next().unwrap_or("").trim() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" => "flac",
        "audio/ogg" => "ogg",
        _ => asked,
    }
}

/// `bytes` with the AI-generated comment written into the file's own
/// metadata: an MP3's ID3v2 tag (COMM) or a WAV's LIST/INFO chunk (ICMT),
/// both of which players and ffprobe read as `comment`. Any other format is
/// returned untouched, with `false`.
///
/// An MP3's existing ID3v2 tag is replaced: what a provider put there is
/// not the owner's, and it can name the vendor.
pub fn tag_ai_generated(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return tag_wav(bytes);
    }
    if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        return (tag_mp3(&bytes), true);
    }
    (bytes, false)
}

fn tag_mp3(bytes: &[u8]) -> Vec<u8> {
    // Skip an existing ID3v2 tag: 10-byte header, a syncsafe size, and a
    // 10-byte footer when its flag says so.
    let mut audio_at = 0;
    if bytes.starts_with(b"ID3") && bytes.len() >= 10 {
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | (*b as usize & 0x7f));
        let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
        audio_at = (10 + size + footer).min(bytes.len());
    }
    // ID3v2.3 COMM: encoding 0 (Latin-1), language "eng", empty
    // description, then the text.
    let mut comm = vec![0u8];
    comm.extend_from_slice(b"eng\0");
    comm.extend_from_slice(AI_TAG.as_bytes());
    let mut frame = b"COMM".to_vec();
    frame.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&comm);
    let size = frame.len();
    let syncsafe = [
        (size >> 21) as u8 & 0x7f,
        (size >> 14) as u8 & 0x7f,
        (size >> 7) as u8 & 0x7f,
        size as u8 & 0x7f,
    ];
    let mut out = b"ID3\x03\x00\x00".to_vec();
    out.extend_from_slice(&syncsafe);
    out.extend_from_slice(&frame);
    out.extend_from_slice(&bytes[audio_at..]);
    out
}

fn tag_wav(mut bytes: Vec<u8>) -> (Vec<u8>, bool) {
    // LIST/INFO/ICMT, inserted before the data chunk so a streamed WAV
    // whose data size runs to the end stays readable.
    let mut text = AI_TAG.as_bytes().to_vec();
    text.push(0);
    if text.len() % 2 == 1 {
        text.push(0);
    }
    let mut info = b"INFO".to_vec();
    info.extend_from_slice(b"ICMT");
    info.extend_from_slice(&((AI_TAG.len() + 1) as u32).to_le_bytes());
    info.extend_from_slice(&text);
    let mut list = b"LIST".to_vec();
    list.extend_from_slice(&(info.len() as u32).to_le_bytes());
    list.extend_from_slice(&info);

    let mut at = 12;
    let data_at = loop {
        if at + 8 > bytes.len() {
            return (bytes, false);
        }
        if &bytes[at..at + 4] == b"data" {
            break at;
        }
        let size =
            u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap_or_default()) as usize;
        at += 8 + size + (size & 1);
    };
    bytes.splice(data_at..data_at, list.iter().copied());
    let riff = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    (bytes, true)
}

/// A transcript as Nebo Media reads it (`docs/publishers-guide/transcript-json.md`).
#[derive(Debug, Serialize, PartialEq)]
pub struct Transcript {
    pub language: String,
    pub duration: f64,
    pub words: Vec<Piece>,
    pub segments: Vec<Piece>,
}

/// One word or one segment: its text, its span in seconds, and its speaker
/// when the audio has more than one.
#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct Piece {
    pub text: String,
    pub start: f64,
    pub end: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Where a segment built from words breaks: a sentence end, a speaker
/// change, a pause this long, or this many words.
const SEGMENT_PAUSE: f64 = 1.0;
const SEGMENT_MAX_WORDS: usize = 16;

/// Janus's `verbose_json` transcription in the transcript shape: words
/// (`text` or `word`) with their speakers relabelled "S1", "S2", … in order
/// of first appearance (dropped when there is only one), and segments as
/// given or, when none came, built from the words.
pub fn transcript(raw: &Value) -> Transcript {
    let secs = |v: Option<&Value>| v.and_then(Value::as_f64).map(ms).unwrap_or(0.0);
    let speaker_key = |v: Option<&Value>| match v {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };
    let mut order: Vec<String> = Vec::new();
    let mut piece = |v: &Value, text_keys: &[&str]| -> Option<Piece> {
        let text = text_keys
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))?
            .trim()
            .to_string();
        if text.is_empty() {
            return None;
        }
        let speaker = speaker_key(v.get("speaker")).map(|k| {
            let i = order.iter().position(|o| *o == k).unwrap_or_else(|| {
                order.push(k);
                order.len() - 1
            });
            format!("S{}", i + 1)
        });
        let start = secs(v.get("start"));
        Some(Piece {
            text,
            start,
            end: secs(v.get("end")).max(start),
            speaker,
        })
    };
    let list = |key: &str| {
        raw.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut words: Vec<Piece> = list("words")
        .iter()
        .filter_map(|w| piece(w, &["text", "word"]))
        .collect();
    let mut segments: Vec<Piece> = list("segments")
        .iter()
        .filter_map(|s| piece(s, &["text"]))
        .collect();
    let duration = secs(raw.get("duration"));
    if segments.is_empty() {
        segments = if words.is_empty() {
            raw.get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| {
                    vec![Piece {
                        text: t.to_string(),
                        start: 0.0,
                        end: duration,
                        speaker: None,
                    }]
                })
                .unwrap_or_default()
        } else {
            segments_of(&words)
        };
    }
    if order.len() < 2 {
        for p in words.iter_mut().chain(segments.iter_mut()) {
            p.speaker = None;
        }
    }
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    Transcript {
        language: raw
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        duration,
        words,
        segments,
    }
}

/// Segments built from words: every word in exactly one, in order.
fn segments_of(words: &[Piece]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut cur: Vec<&Piece> = Vec::new();
    let flush = |cur: &mut Vec<&Piece>, out: &mut Vec<Piece>| {
        if let (Some(first), Some(last)) = (cur.first(), cur.last()) {
            out.push(Piece {
                text: cur
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                start: first.start,
                end: last.end,
                speaker: first.speaker.clone(),
            });
        }
        cur.clear();
    };
    for w in words {
        if let Some(prev) = cur.last()
            && (prev.speaker != w.speaker
                || w.start - prev.end >= SEGMENT_PAUSE
                || cur.len() >= SEGMENT_MAX_WORDS)
        {
            flush(&mut cur, &mut out);
        }
        cur.push(w);
        if w.text.ends_with(['.', '?', '!']) {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Seconds to the millisecond.
fn ms(s: f64) -> f64 {
    (s.max(0.0) * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: usize) -> Vec<u8> {
        let data = vec![0u8; seconds * 16_000];
        let mut b = b"RIFF".to_vec();
        b.extend(((36 + data.len()) as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend([1, 0, 1, 0]);
        b.extend(8_000u32.to_le_bytes());
        b.extend(16_000u32.to_le_bytes());
        b.extend([2, 0, 16, 0]);
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn music_is_instrumental_unless_it_has_lyrics() {
        let b = generation_body("music", &json!({"prompt": "lofi", "seconds": 900}), "mp3");
        assert_eq!(
            b,
            json!({"kind": "music", "prompt": "lofi", "response_format": "mp3", "seconds": 300, "instrumental": true})
        );
        let b = generation_body(
            "music",
            &json!({"prompt": "pop", "lyrics": "[Verse]\nhey"}),
            "wav",
        );
        assert_eq!(b["instrumental"], false);
        assert_eq!(b["lyrics"], "[Verse]\nhey");
        let b = generation_body(
            "music",
            &json!({"prompt": "pop", "lyrics": "x", "instrumental": true}),
            "mp3",
        );
        assert!(b.get("lyrics").is_none(), "{b}");
        let b = generation_body(
            "sound",
            &json!({"prompt": "door creak", "seconds": 0, "lyrics": "x", "model": "m"}),
            "mp3",
        );
        assert_eq!(
            b,
            json!({"kind": "sound", "prompt": "door creak", "response_format": "mp3", "seconds": 1, "model": "m"})
        );
    }

    #[test]
    fn audio_ext_trusts_the_bytes_first() {
        assert_eq!(audio_ext(&wav(1), "audio/mpeg", "mp3"), "wav");
        assert_eq!(audio_ext(b"ID3\x03", "", "wav"), "mp3");
        assert_eq!(audio_ext(b"????", "audio/flac", "mp3"), "flac");
        assert_eq!(audio_ext(b"????", "", "mp3"), "mp3");
    }

    /// The tag lands in the file's own metadata, where ffprobe reads it as
    /// `comment`, and the audio itself is untouched.
    #[test]
    fn made_audio_is_tagged_ai_generated() {
        // MP3: a provider tag is replaced by ours; the frames follow intact.
        let frames = [0xffu8, 0xfb, 0x90, 0x64, 1, 2, 3];
        let mut mp3 = b"ID3\x04\0\0\0\0\0\x05VENDR".to_vec();
        mp3.extend(frames);
        let (tagged, ok) = tag_ai_generated(mp3);
        assert!(ok);
        assert!(tagged.starts_with(b"ID3\x03\0\0"));
        let size = tagged[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | *b as usize);
        assert_eq!(&tagged[10 + size..], &frames);
        assert!(tagged.windows(4).any(|w| w == b"COMM"));
        assert!(tagged.windows(AI_TAG.len()).any(|w| w == AI_TAG.as_bytes()));
        assert!(
            !tagged.windows(5).any(|w| w == b"VENDR"),
            "the provider's tag is gone"
        );

        // WAV: a LIST/INFO/ICMT chunk before the data, the RIFF size kept true.
        let plain = wav(1);
        let (tagged, ok) = tag_ai_generated(plain.clone());
        assert!(ok);
        assert_eq!(
            u32::from_le_bytes(tagged[4..8].try_into().unwrap()) as usize,
            tagged.len() - 8
        );
        let list = tagged.windows(4).position(|w| w == b"LIST").unwrap();
        let data = tagged.windows(4).position(|w| w == b"data").unwrap();
        assert!(list < data);
        assert_eq!(&tagged[list + 8..list + 16], b"INFOICMT");
        assert_eq!(
            tagged[data..],
            plain[plain.windows(4).position(|w| w == b"data").unwrap()..]
        );

        let (same, ok) = tag_ai_generated(b"fLaC....".to_vec());
        assert!(!ok && same == b"fLaC....");
    }

    #[test]
    fn transcript_relabels_speakers_and_builds_segments() {
        let raw = json!({
            "text": "Speaker 1: Hi there. Speaker 2: Who is this",
            "language": "en",
            "duration": 3.14159,
            "words": [
                {"text": "Hi", "start": 0.0, "end": 0.3, "speaker": 1},
                {"text": "there.", "start": 0.3, "end": 0.6, "speaker": 1},
                {"text": "Who", "start": 0.8, "end": 1.0, "speaker": 0},
                {"text": " ", "start": 1.0, "end": 1.0, "speaker": 0},
                {"text": "is", "start": 1.0, "end": 1.1, "speaker": 0},
                {"text": "this", "start": 2.5, "end": 2.8, "speaker": 0}
            ]
        });
        let t = transcript(&raw);
        assert_eq!(t.language, "en");
        assert_eq!(t.duration, 3.142);
        assert_eq!(t.words.len(), 5, "blank words dropped");
        assert_eq!(
            t.words[0].speaker.as_deref(),
            Some("S1"),
            "labels follow first appearance"
        );
        assert_eq!(t.words[2].speaker.as_deref(), Some("S2"));
        let segs: Vec<(&str, f64, f64, Option<&str>)> = t
            .segments
            .iter()
            .map(|s| (s.text.as_str(), s.start, s.end, s.speaker.as_deref()))
            .collect();
        assert_eq!(
            segs,
            [
                ("Hi there.", 0.0, 0.6, Some("S1")),
                ("Who is", 0.8, 1.1, Some("S2")),
                ("this", 2.5, 2.8, Some("S2"))
            ]
        );
        let json = serde_json::to_value(&t).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(keys, ["duration", "language", "segments", "words"]);
    }

    #[test]
    fn one_voice_has_no_speakers_and_given_segments_are_kept() {
        // The OpenAI word shape (`word`), one speaker, segments supplied.
        let raw = json!({
            "language": "english", "duration": 2.0,
            "words": [{"word": "Hello", "start": 0.1, "end": 0.4, "speaker": "A"}, {"word": "world", "start": 0.5, "end": 0.9, "speaker": "A"}],
            "segments": [{"id": 0, "text": " Hello world ", "start": 0.1, "end": 0.9, "speaker": "A"}]
        });
        let t = transcript(&raw);
        assert!(
            t.words
                .iter()
                .chain(&t.segments)
                .all(|p| p.speaker.is_none())
        );
        assert_eq!(
            t.segments,
            [Piece {
                text: "Hello world".into(),
                start: 0.1,
                end: 0.9,
                speaker: None
            }]
        );
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("speaker"), "{json}");

        // Text only: one segment over the whole file.
        let t = transcript(&json!({"text": "Just text.", "duration": 4.0}));
        assert!(t.words.is_empty());
        assert_eq!(
            t.segments,
            [Piece {
                text: "Just text.".into(),
                start: 0.0,
                end: 4.0,
                speaker: None
            }]
        );
    }
}
