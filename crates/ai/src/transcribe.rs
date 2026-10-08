//! Speech-to-text for audio *files*.
//!
//! Distinct from `nebo-voice`, which is a live duplex stream — this is the
//! one-shot path for an audio attachment that arrives in a message. It speaks
//! the OpenAI `/audio/transcriptions` shape, which xAI, Groq, and the local
//! whisper servers all implement as well, so the endpoint is a base URL rather
//! than a provider enum.

use crate::{ProviderError, RequestTrace};

/// Providers reject anything larger, and the request would be a slow way to
/// find that out.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// Formats the endpoint accepts. Kept in step with Janus's `audio.stt.extensions`
/// allowlist, which is matched verbatim — a format listed here but not there is
/// rejected at the gateway before the audio is ever read.
pub const SUPPORTED_AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "mp3", "mp4", "mpeg", "mpga", "ogg", "wav", "webm",
];

/// Whether this filename/MIME pair looks like audio we can transcribe.
pub fn is_transcribable(filename: &str, mime_type: &str) -> bool {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(ext) if SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str()) => true,
        // A voice note recorded in the browser often arrives with a generic or
        // absent extension, so the declared type still gets a say.
        _ => mime_type.starts_with("audio/"),
    }
}

/// Transcribe an audio file. Returns the spoken text.
///
/// An empty transcript is returned as `Ok("")` — silence is a real answer, and
/// the caller says so in words rather than presenting nothing. Sound with no
/// speech in it is silence too: what the transcriber wrote for it is dropped
/// ([`speech_in`]). Live 2026-10-08: eight phone clips of a warehouse came back
/// as "📢 Share this video with your friends on social media.",
/// "Продолжение следует...", "시청해주셔서 감사합니다!" and the like.
pub async fn transcribe(
    trace: &RequestTrace,
    api_key: &str,
    base_url: &str,
    model: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<String, ProviderError> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(ProviderError::Request(format!(
            "audio file is {:.1} MB; the transcription limit is {} MB",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_AUDIO_BYTES / (1024 * 1024)
        )));
    }

    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
    let form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "verbose_json")
        .part("file", part);

    let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
    let response = tls::http_client()
        .user_agent(types::constants::USER_AGENT)
        .build()
        .map_err(|e| ProviderError::Request(e.to_string()))?
        .post(&url)
        .bearer_auth(api_key)
        .headers(trace.headers())
        .multipart(form)
        .send()
        .await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ProviderError::Request(e.to_string()))?;

    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => ProviderError::Auth(body),
            429 => ProviderError::RateLimit { retry_after_secs: None },
            _ => ProviderError::Api {
                code: status.as_u16().to_string(),
                message: body,
                retryable: status.is_server_error(),
            },
        });
    }

    Ok(speech_in(&body))
}

/// A segment the transcriber itself rates this likely to hold no speech is
/// not speech, whatever words it wrote for it.
const NO_SPEECH: f64 = 0.6;
/// Below [`NO_SPEECH`], a segment it was also this unsure of is not speech
/// either (Whisper's own pairing of the two).
const UNSURE_NO_SPEECH: f64 = 0.3;
const UNSURE_LOGPROB: f64 = -1.0;
/// A segment this repetitive is the decoder looping, not someone talking.
const LOOPING_COMPRESSION: f64 = 2.4;

/// The speech in a transcription response: the text, without the segments
/// the transcriber rated as no speech, and empty when all that is left is
/// one of the lines transcribers invent for non-speech audio. A response
/// that is not JSON is taken as the text itself.
pub fn speech_in(body: &str) -> String {
    let text = match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(v) if v.is_object() => spoken_segments(&v).unwrap_or_else(|| v["text"].as_str().unwrap_or_default().trim().to_string()),
        _ => body.trim().to_string(),
    };
    if invented(&text) { String::new() } else { text }
}

/// The text of the segments that hold speech, when the response rates its
/// segments (`no_speech_prob`); `None` when it does not.
fn spoken_segments(v: &serde_json::Value) -> Option<String> {
    let segments = v["segments"].as_array()?;
    if segments.is_empty() || !segments.iter().any(|s| s["no_speech_prob"].is_number()) {
        return None;
    }
    let spoken: Vec<&str> = segments
        .iter()
        .filter(|s| {
            let no_speech = s["no_speech_prob"].as_f64().unwrap_or(0.0);
            let logprob = s["avg_logprob"].as_f64().unwrap_or(0.0);
            let compression = s["compression_ratio"].as_f64().unwrap_or(0.0);
            no_speech < NO_SPEECH
                && !(no_speech >= UNSURE_NO_SPEECH && logprob < UNSURE_LOGPROB)
                && compression <= LOOPING_COMPRESSION
        })
        .filter_map(|s| s["text"].as_str().map(str::trim))
        .filter(|t| !t.is_empty())
        .collect();
    Some(spoken.join(" "))
}

/// Lines speech-to-text models write for music, noise and silence: the
/// sign-offs and subtitle credits of the videos they learned from. Each is
/// matched as whole words, ignoring case and punctuation.
const INVENTED_LINES: &[&str] = &[
    "thank you for watching",
    "thanks for watching",
    "thank you so much for watching",
    "thank you for watching and see you next time",
    "please subscribe",
    "please like and subscribe",
    "like and subscribe",
    "subscribe to my channel",
    "don't forget to like and subscribe",
    "share this video with your friends on social media",
    "see you in the next video",
    "subtitles by the amara.org community",
    "transcribed by https://otter.ai",
    "продолжение следует",
    "спасибо за просмотр",
    "субтитры сделал dimatorzok",
    "субтитры создавал dimatorzok",
    "редактор субтитров а.синецкая корректор а.егорова",
    "시청해주셔서 감사합니다",
    "구독과 좋아요 부탁드립니다",
    "ご視聴ありがとうございました",
    "チャンネル登録をお願いします",
    "字幕由amara.org社区提供",
    "请不吝点赞 订阅 转发 打赏支持明镜与点点栏目",
    "untertitel im auftrag des zdf",
    "untertitel der amara.org-community",
    "sous-titres réalisés par la communauté d'amara.org",
    "sous-titrage st' 501",
    "sottotitoli creati dalla comunità amara.org",
    "subtítulos realizados por la comunidad de amara.org",
    "gracias por ver el video",
    "obrigado por assistir",
];

/// Lowercased words, punctuation and symbols (an emoji, "...") dropped.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Whether the transcript is nothing but [`INVENTED_LINES`], once or over
/// and over. A transcript with any other word in it is kept whole.
fn invented(text: &str) -> bool {
    let said = words(text);
    if said.is_empty() {
        return false;
    }
    let mut lines: Vec<Vec<String>> = INVENTED_LINES.iter().map(|l| words(l)).collect();
    lines.sort_by_key(|l| std::cmp::Reverse(l.len()));
    let mut at = 0;
    while at < said.len() {
        match lines.iter().find(|l| said[at..].starts_with(l)) {
            Some(line) => at += line.len(),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four clips of 2026-10-08, transcribed as text: each is a line a
    /// transcriber invents, so each is no speech.
    #[test]
    fn invented_lines_alone_are_no_speech() {
        for body in [
            "📢 Share this video with your friends on social media.",
            "Продолжение следует...",
            "시청해주셔서 감사합니다!",
            "Thank you for watching! Thank you for watching!",
            "  Thanks for watching.\n",
        ] {
            assert_eq!(speech_in(body), "", "{body}");
        }
    }

    #[test]
    fn speech_that_holds_an_invented_line_is_kept_whole() {
        let said = "Thanks for watching the warehouse while I was out. Share this video with your friends on social media.";
        assert_eq!(speech_in(said), said);
        assert_eq!(speech_in("Thank you."), "Thank you.", "a short real note is not on the list");
    }

    /// A segment the transcriber rates as no speech is dropped, words and
    /// all; the speech beside it stays.
    #[test]
    fn segments_rated_no_speech_are_dropped() {
        let body = serde_json::json!({
            "text": "You see our magnificent warehouse. I hope you appreciate it.",
            "segments": [
                {"text": " You see our magnificent warehouse.", "no_speech_prob": 0.02, "avg_logprob": -0.21, "compression_ratio": 1.1},
                {"text": " I hope you appreciate it.", "no_speech_prob": 0.71, "avg_logprob": -0.4, "compression_ratio": 1.0},
            ]
        })
        .to_string();
        assert_eq!(speech_in(&body), "You see our magnificent warehouse.");

        let ambient = serde_json::json!({
            "text": "I hope you appreciate it. I'll wrap it up now.",
            "segments": [
                {"text": "I hope you appreciate it.", "no_speech_prob": 0.45, "avg_logprob": -1.3, "compression_ratio": 1.0},
                {"text": "I'll wrap it up now.", "no_speech_prob": 0.9, "avg_logprob": -0.3, "compression_ratio": 1.0},
            ]
        })
        .to_string();
        assert_eq!(speech_in(&ambient), "", "no segment holds speech");

        let looping = serde_json::json!({
            "text": "you you you",
            "segments": [{"text": "you you you you you you you you", "no_speech_prob": 0.1, "avg_logprob": -0.5, "compression_ratio": 3.2}]
        })
        .to_string();
        assert_eq!(speech_in(&looping), "");
    }

    /// A response with no ratings (another provider's JSON, or plain text)
    /// keeps its text, still checked for invented lines.
    #[test]
    fn a_response_without_ratings_keeps_its_text() {
        assert_eq!(speech_in(r#"{"text": " Order 4471 ships Friday. "}"#), "Order 4471 ships Friday.");
        assert_eq!(speech_in(r#"{"text": "Продолжение следует...", "segments": [{"text": "Продолжение следует...", "start": 0, "end": 2}]}"#), "");
        assert_eq!(speech_in("Order 4471 ships Friday."), "Order 4471 ships Friday.");
        assert_eq!(speech_in(""), "");
    }
}
