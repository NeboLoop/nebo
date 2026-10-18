//! Voice — speech-to-speech conversation via the xAI Grok realtime API, and
//! dictation through NeboAI's streaming transcription.
//!
//! The local pipeline (whisper STT cascade + Kokoro TTS) was removed:
//! conversation was structurally too slow as a serialized STT → LLM → TTS
//! cascade. Desktop dictation is the OS's own (macOS dictation, Win+H)
//! typing straight into the composer; the phone app dictates through the
//! bot (`dictation`), which streams its microphone to the metered `/v1/stt`
//! relay and hands back live, punctuated text with no time limit per request.
//!
//! What remains is the realtime protocol client (`realtime`), the shared
//! event vocabulary (`conversation`) consumed by the server's
//! `/ws/voice/conversation` relay, and the dictation relay (`dictation`)
//! behind `/ws/voice/dictation`.

pub mod conversation;
pub mod dictation;
pub mod realtime;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("realtime voice error: {0}")]
    Realtime(String),
}

/// Most bias terms the recognizer takes, voice call or dictation.
pub const MAX_KEYTERMS: usize = 100;

/// Longest bias term the recognizer takes, in characters.
pub const MAX_KEYTERM_CHARS: usize = 50;

/// The words the recognizer should expect, as it takes them: each trimmed,
/// empty and over-long ones dropped, the first spelling of a word kept when
/// it comes again in any case, at most [`MAX_KEYTERMS`]. `terms` comes most
/// important first, so the cap drops the least.
pub fn keyterms<I, S>(terms: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = std::collections::HashSet::new();
    terms
        .into_iter()
        .map(|t| t.as_ref().trim().to_string())
        .filter(|t| !t.is_empty() && t.chars().count() <= MAX_KEYTERM_CHARS)
        .filter(|t| seen.insert(t.to_lowercase()))
        .take(MAX_KEYTERMS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyterms_are_trimmed_deduped_bounded_and_kept_in_order() {
        let long = "x".repeat(MAX_KEYTERM_CHARS + 1);
        let fits = "é".repeat(MAX_KEYTERM_CHARS);
        let got = keyterms(["  Nebo ", "NeboAI", "nebo", "", "   ", long.as_str(), fits.as_str(), "Tymick", "NEBOAI"]);
        assert_eq!(got, vec!["Nebo".to_string(), "NeboAI".into(), fits, "Tymick".into()]);

        let many: Vec<String> = (0..150).map(|i| format!("Term {i}")).collect();
        let got = keyterms(&many);
        assert_eq!(got.len(), MAX_KEYTERMS);
        assert_eq!(got.first().map(String::as_str), Some("Term 0"));
        assert_eq!(got.last().map(String::as_str), Some("Term 99"));
    }
}
